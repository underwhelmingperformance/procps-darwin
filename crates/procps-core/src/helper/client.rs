// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{io, os::unix::net::UnixStream, path::PathBuf, time::Duration};

use darwin_proc::{PeerCredentials, Uid};
use rustix::{
    io::{FdFlags, fcntl_setfd},
    net::{
        AddressFamily, SocketAddrUnix, SocketType, connect, socket, sockopt::set_socket_nosigpipe,
    },
};

use crate::{
    ProcessSource, Snapshot, SnapshotRequest, SourceError,
    helper::{Deadline, ProtocolError, ReadMessage, Refusal, Request, Response, WriteMessage},
};

/// The path of the helper's socket.
///
/// ```
/// assert!(procps_core::helper::SOCKET.starts_with("/var/run/"));
/// ```
pub const SOCKET: &str = "/var/run/procps-darwin/helper.sock";

/// How long a tool waits for the whole exchange with the helper. Keep it above
/// the helper's time limits, whose defaults add up to 21 seconds, so that the
/// tool receives the helper's response or refusal before its own time runs out.
const PATIENCE: Duration = Duration::from_secs(30);

/// Takes snapshots through the helper daemon, which reads other users'
/// processes as root.
///
/// Before it sends a request, the source checks that the process listening on
/// the socket runs as root, so that the tool never reads data from a socket
/// that another user has created at the path.
///
/// ```
/// use procps_core::{ProcessSource, SnapshotRequest, helper::HelperSource};
///
/// let source = HelperSource::new("/nonexistent/helper.sock");
///
/// assert!(source.snapshot(&SnapshotRequest::default()).is_err());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperSource {
    path: PathBuf,
    owner: Uid,
    patience: Duration,
}

/// An error from taking a snapshot through the helper.
#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    /// The tool could not connect to the helper's socket.
    #[error("cannot connect to the helper")]
    Connect(#[from] io::Error),
    /// The tool could not identify the process that listens on the socket.
    #[error("cannot identify the helper")]
    Peer(#[from] darwin_proc::Error),
    /// The process that listens on the socket runs as another user.
    #[error(
        "the process listening on the helper's socket runs as user {}, not user {}",
        found.as_raw(),
        expected.as_raw()
    )]
    Untrusted {
        /// The effective user ID of the process that listens on the socket.
        found: Uid,
        /// The user ID that the tool requires.
        expected: Uid,
    },
    /// The tool could not exchange messages with the helper.
    #[error("cannot exchange messages with the helper")]
    Protocol(#[from] ProtocolError),
    /// The helper refused the request.
    #[error("the helper refused the request")]
    Refused(#[source] Refusal),
}

impl HelperError {
    /// Whether the helper will fail in the same way for every later request:
    /// its socket is missing or untrusted, the tool cannot identify the process
    /// listening on it, or the helper uses another version of the protocol.
    ///
    /// A refused connection is not persistent, because Darwin refuses
    /// connections to a socket whose listen backlog is full.
    ///
    /// ```
    /// use procps_core::helper::{HelperError, Refusal};
    ///
    /// assert!(!HelperError::Refused(Refusal::Busy).is_persistent());
    /// ```
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        match self {
            Self::Connect(error) => error.kind() == io::ErrorKind::NotFound,
            Self::Untrusted { .. }
            | Self::Peer(_)
            | Self::Protocol(ProtocolError::Version { .. }) => true,
            Self::Protocol(_) | Self::Refused(_) => false,
        }
    }
}

impl HelperSource {
    /// A source that connects to the socket at `path`. A process running as
    /// root must listen on the socket.
    ///
    /// ```
    /// use procps_core::{
    ///     ProcessSource, SnapshotRequest, SourceError,
    ///     helper::{HelperError, HelperSource},
    /// };
    ///
    /// let error = HelperSource::new("/nonexistent/helper.sock").snapshot(&SnapshotRequest::default());
    ///
    /// assert!(matches!(
    ///     error,
    ///     Err(SourceError::Helper(HelperError::Connect(_)))
    /// ));
    /// ```
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            owner: Uid::from(0),
            patience: PATIENCE,
        }
    }

    /// Returns a copy of this source that accepts the socket only when the
    /// process listening on it runs as `owner`. Tests call this method to
    /// connect to a helper that runs as the test's own user.
    ///
    /// ```
    /// use darwin_proc::Uid;
    /// use procps_core::{
    ///     ProcessSource, SnapshotRequest, SourceError,
    ///     helper::{HelperError, HelperSource},
    /// };
    ///
    /// let source = HelperSource::new("/nonexistent/helper.sock").owned_by(Uid::from(501));
    ///
    /// assert!(matches!(
    ///     source.snapshot(&SnapshotRequest::default()),
    ///     Err(SourceError::Helper(HelperError::Connect(_)))
    /// ));
    /// ```
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn owned_by(self, owner: Uid) -> Self {
        Self { owner, ..self }
    }

    /// Returns a copy of this source that waits `patience` for the whole
    /// exchange with the helper. Tests call this method to time out quickly.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use procps_core::{ProcessSource, SnapshotRequest, helper::HelperSource};
    ///
    /// let source = HelperSource::new("/nonexistent/helper.sock").patience(Duration::from_millis(10));
    ///
    /// assert!(source.snapshot(&SnapshotRequest::default()).is_err());
    /// ```
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn patience(self, patience: Duration) -> Self {
        Self { patience, ..self }
    }

    /// Connects to the helper's socket.
    fn connect(&self) -> io::Result<UnixStream> {
        let stream = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
        fcntl_setfd(&stream, FdFlags::CLOEXEC)?;

        // Set the option before connecting. The helper may close its end after
        // a refusal sent before it reads the whole request, and Darwin then
        // refuses to set the option.
        set_socket_nosigpipe(&stream, true)?;
        connect(&stream, &SocketAddrUnix::new(&self.path)?)?;

        Ok(stream.into())
    }

    /// Sends `request` to the helper and receives the snapshot.
    fn request(&self, request: &SnapshotRequest) -> Result<Snapshot, HelperError> {
        let stream = self.connect()?;

        // Use `PeerCredentials`, not `Peer::of`. The helper may already have
        // closed its end after a refusal, and `LOCAL_PEERPID` then fails.
        let found = PeerCredentials::of(&stream)?.uid;

        if found != self.owner {
            return Err(HelperError::Untrusted {
                found,
                expected: self.owner,
            });
        }

        let mut connection = Deadline::new(&stream, self.patience);

        match connection.write_message(&Request::Snapshot(request.clone())) {
            Ok(()) => {}
            // The helper closes the connection after a refusal without
            // reading the rest of the request, and the refusal, or a reply in
            // another protocol version, may already be in the receive buffer.
            // Darwin then fails the write with either of these errors.
            Err(ProtocolError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected
                ) =>
            {
                return Err(match connection.read_message::<Response>() {
                    Ok(Response::Refused(refusal)) => HelperError::Refused(refusal),
                    Ok(Response::Snapshot(_)) | Err(ProtocolError::Io(_)) => {
                        ProtocolError::Io(error).into()
                    }
                    Err(reply) => reply.into(),
                });
            }
            Err(error) => return Err(error.into()),
        }

        match connection.read_message::<Response>()? {
            Response::Snapshot(snapshot) => Ok(*snapshot),
            Response::Refused(refusal) => Err(HelperError::Refused(refusal)),
        }
    }
}

impl ProcessSource for HelperSource {
    #[tracing::instrument(level = "debug", skip_all, fields(path = %self.path.display()), err(level = "debug"))]
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        Ok(self.request(request)?)
    }
}
