// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::CStr,
    fmt,
    os::unix::{
        ffi::OsStrExt,
        net::{SocketAddr, UnixListener},
    },
    path::PathBuf,
    time::Duration,
};

use darwin_proc::LaunchdSockets;
use procps_core::LocalSource;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use crate::{ConnectionLimits, ErrorChain, ListenError, Listener, Server, TimeLimits};

/// The name of the entry in the `Sockets` dictionary of the helper's launchd
/// property list.
const SOCKETS: &CStr = c"Listeners";

/// How the helper runs.
///
/// ```
/// use std::time::Duration;
///
/// use procps_helperd::Config;
///
/// assert_eq!(Config::default().idle, Duration::from_secs(60));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// A path to bind a listening socket at, in place of the socket that
    /// launchd passes.
    pub socket: Option<PathBuf>,
    /// How long the helper keeps running after it starts or after its last
    /// connection closes, whichever is later.
    pub idle: Duration,
    /// The time limits for each connection.
    pub time_limits: TimeLimits,
    /// The most connections that the helper keeps open at once.
    pub connections: ConnectionLimits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket: None,
            idle: Duration::from_secs(60),
            time_limits: TimeLimits::default(),
            connections: ConnectionLimits::default(),
        }
    }
}

/// An error that stops the helper.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// `launch_activate_socket` failed, for example because launchd did not
    /// start the helper.
    #[error("cannot take the socket from launchd")]
    Launchd(#[from] darwin_proc::Error),
    /// launchd passed a number of sockets other than one.
    #[error("launchd passed {0} sockets instead of 1")]
    Sockets(usize),
    /// The helper could not listen on the path in [`Config::socket`].
    #[error("cannot listen on the socket")]
    Bind(#[from] std::io::Error),
    /// The helper stopped accepting connections.
    #[error(transparent)]
    Listen(#[from] ListenError),
}

impl Config {
    /// Serves process data from [`LocalSource`] on the socket that this
    /// configuration chooses, until the helper is idle.
    ///
    /// ```
    /// use procps_helperd::{Config, DaemonError};
    ///
    /// // launchd did not start the doctest, so `launch_activate_socket` fails.
    /// assert!(matches!(
    ///     Config::default().run(),
    ///     Err(DaemonError::Launchd(_))
    /// ));
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`DaemonError`] if the helper cannot get its socket, or stops
    /// accepting connections because of an error.
    pub fn run(&self) -> Result<(), DaemonError> {
        let socket = match &self.socket {
            Some(path) => UnixListener::bind(path)?,
            None => match <[_; 1]>::try_from(LaunchdSockets::activate(SOCKETS)?) {
                Ok([socket]) => UnixListener::from(socket),
                Err(sockets) => return Err(DaemonError::Sockets(sockets.len())),
            },
        };
        self.raise_descriptor_limit();
        let server = Server::new(LocalSource, self.time_limits);

        match socket.local_addr() {
            Ok(address) => tracing::info!(address = %Address(&address), "listening"),
            Err(error) => {
                tracing::warn!(error = %ErrorChain(&error), "cannot read the listening address");
            }
        }
        Listener::new(server, self.idle, self.connections).run(&socket)?;

        Ok(())
    }
}

impl Config {
    /// Raises the soft limit on open file descriptors to what the connection
    /// limits need, or to the hard limit if that is lower. launchd's default
    /// soft limit is 256, which the helper would reach before it could refuse
    /// a connection with `Busy`. If the hard limit is lower than the connection
    /// limits need, or `setrlimit` fails, the helper logs a warning.
    fn raise_descriptor_limit(&self) {
        let limit = getrlimit(Resource::Nofile);
        let wanted = self.connections.descriptors();
        let enough = limit.current.is_none_or(|current| current >= wanted);

        if enough {
            return;
        }

        let raised = Rlimit {
            current: Some(limit.maximum.map_or(wanted, |maximum| maximum.min(wanted))),
            ..limit
        };

        if raised.current < Some(wanted) {
            tracing::warn!(
                ?limit,
                wanted,
                "the hard limit on open files is below what the connection limits need"
            );
        }

        if let Err(error) = setrlimit(Resource::Nofile, raised) {
            tracing::warn!(
                error = %ErrorChain(&error),
                ?limit,
                wanted,
                "cannot raise the limit on open files"
            );
        }
    }
}

/// A socket's path for the logs, up to the first NUL. The address of the
/// socket that launchd passes covers the whole of `sun_path`, so the path is
/// followed by NUL padding.
struct Address<'a>(&'a SocketAddr);

impl fmt::Display for Address<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(path) = self.0.as_pathname() else {
            return formatter.write_str("an unnamed socket");
        };
        let bytes = path.as_os_str().as_bytes();
        let end = bytes
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(bytes.len());

        write!(formatter, "{}", String::from_utf8_lossy(&bytes[..end]))
    }
}
