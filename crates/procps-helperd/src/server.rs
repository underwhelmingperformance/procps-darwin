// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io,
    num::NonZeroUsize,
    os::unix::net::UnixStream,
    thread,
    time::{Duration, Instant},
};

use darwin_proc::Peer;
use procps_core::{
    ErrorChain, ProcessSource, SnapshotRequest,
    helper::{
        Caller, Deadline, ProtocolError, ReadMessage, Refusal, Request, Response, WriteMessage,
    },
};

use crate::workers::Workers;

/// The time limits for each connection.
///
/// ```
/// use std::time::Duration;
///
/// use procps_helperd::TimeLimits;
///
/// assert_eq!(TimeLimits::default().snapshot, Duration::from_secs(10));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeLimits {
    /// The time that a client has to send its request after it connects. A
    /// request is at most 1 MiB, so a client that sends it at once needs far
    /// less.
    pub request: Duration,
    /// The time that the helper has to read the process data after it
    /// receives the request, including any wait for a worker.
    pub snapshot: Duration,
    /// The time that a client has to receive the response.
    pub response: Duration,
}

impl TimeLimits {
    /// The longest that a connection can take within its limits.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use procps_helperd::TimeLimits;
    ///
    /// assert_eq!(TimeLimits::default().total(), Duration::from_secs(21));
    /// ```
    #[must_use]
    pub const fn total(&self) -> Duration {
        self.request
            .saturating_add(self.snapshot)
            .saturating_add(self.response)
    }
}

impl Default for TimeLimits {
    fn default() -> Self {
        Self {
            request: Duration::from_secs(1),
            snapshot: Duration::from_secs(10),
            response: Duration::from_secs(10),
        }
    }
}

/// Answers one request on each connection with a snapshot from a
/// [`ProcessSource`].
///
/// The server has one worker for each processor, and each worker takes one
/// snapshot at a time, because a snapshot keeps a processor busy. The wait for
/// a worker counts towards the request's snapshot time limit.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use procps_core::FixtureSource;
/// use procps_helperd::{Server, TimeLimits};
///
/// let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
///
/// assert_eq!(
///     Server::new(source, TimeLimits::default()).limits(),
///     TimeLimits::default()
/// );
/// ```
#[derive(Debug)]
pub struct Server<S> {
    source: S,
    limits: TimeLimits,
    workers: Workers,
}

/// An error that ends a connection before the helper can send a response.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The helper could not identify the client, usually because the client
    /// has closed the connection.
    #[error("cannot identify the client")]
    Peer(#[from] darwin_proc::Error),
    /// The connection failed.
    #[error("cannot exchange messages with the client")]
    Protocol(#[from] ProtocolError),
}

impl<S: ProcessSource> Server<S> {
    /// A server that reads process data from `source`, within `limits`.
    ///
    /// ```
    /// use procps_core::LocalSource;
    /// use procps_helperd::{Server, TimeLimits};
    ///
    /// let server = Server::new(LocalSource, TimeLimits::default());
    /// # let _ = server;
    /// ```
    #[must_use]
    pub fn new(source: S, limits: TimeLimits) -> Self {
        let processors = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);

        Self {
            source,
            limits,
            workers: Workers::new(processors),
        }
    }

    /// This server, taking at most `workers` snapshots at once.
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    ///
    /// use procps_core::LocalSource;
    /// use procps_helperd::{Server, TimeLimits};
    ///
    /// let server = Server::new(LocalSource, TimeLimits::default()).with_workers(NonZeroUsize::MIN);
    /// # let _ = server;
    /// ```
    #[must_use]
    pub fn with_workers(self, workers: NonZeroUsize) -> Self {
        Self {
            workers: Workers::new(workers),
            ..self
        }
    }

    /// The server's time limits.
    ///
    /// ```
    /// use procps_core::LocalSource;
    /// use procps_helperd::{Server, TimeLimits};
    ///
    /// assert_eq!(
    ///     Server::new(LocalSource, TimeLimits::default()).limits(),
    ///     TimeLimits::default()
    /// );
    /// ```
    #[must_use]
    pub const fn limits(&self) -> TimeLimits {
        self.limits
    }

    /// Reads one request from `stream`, and writes the response, which is a
    /// snapshot or a [`Refusal`].
    ///
    /// ```
    /// use std::{
    ///     os::unix::net::UnixStream,
    ///     thread,
    ///     time::{Duration, SystemTime},
    /// };
    ///
    /// use procps_core::{
    ///     FixtureSource, SnapshotRequest,
    ///     helper::{ReadMessage, Request, Response, WriteMessage},
    /// };
    /// use procps_helperd::{Server, TimeLimits};
    ///
    /// let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
    /// let server = Server::new(source, TimeLimits::default());
    /// let (mut client, theirs) = UnixStream::pair()?;
    ///
    /// client.write_message(&Request::Snapshot(SnapshotRequest::default()))?;
    /// thread::spawn(move || server.serve(&theirs));
    ///
    /// assert!(matches!(
    ///     client.read_message::<Response>()?,
    ///     Response::Snapshot(_)
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ServeError`] if the helper cannot identify the client, or if
    /// reading or writing the socket fails, for example because the client
    /// closed the connection.
    pub fn serve(&self, stream: &UnixStream) -> Result<(), ServeError> {
        self.serve_peer(stream, Peer::of(stream)?)
    }

    /// Serves `stream`, whose client is `peer`, as [`Server::serve`] does.
    #[tracing::instrument(
        level = "info",
        skip_all,
        fields(uid = peer.uid.as_raw(), gid = peer.gid.as_raw(), pid = peer.pid.as_raw())
    )]
    pub(crate) fn serve_peer(&self, stream: &UnixStream, peer: Peer) -> Result<(), ServeError> {
        let started = Instant::now();
        let caller = Caller::new(peer.uid, peer.gid);
        let response = match Deadline::new(stream, self.limits.request).read_message::<Request>() {
            Ok(Request::Snapshot(request)) => self.snapshot(caller, peer, &request),
            Err(error) => Response::Refused(refusal(error)?),
        };

        match self.respond(stream, response)? {
            Response::Snapshot(snapshot) => tracing::debug!(
                processes = snapshot.processes.len(),
                elapsed = ?started.elapsed(),
                "sent a snapshot"
            ),
            Response::Refused(refusal) => tracing::debug!(
                ?refusal,
                elapsed = ?started.elapsed(),
                "refused the request"
            ),
        }

        Ok(())
    }

    /// Sends [`Refusal::Busy`] on `stream`, to a client that the helper will
    /// not serve because too many connections are open.
    pub(crate) fn refuse_busy(&self, stream: &UnixStream) -> Result<(), ServeError> {
        Deadline::new(stream, self.limits.request)
            .write_message(&Response::Refused(Refusal::Busy))?;

        Ok(())
    }

    /// The snapshot that `request` asks for, with private values withheld from
    /// `caller`, or a refusal.
    fn snapshot(&self, caller: Caller, peer: Peer, request: &SnapshotRequest) -> Response {
        let started = Instant::now();
        let until = started.checked_add(self.limits.snapshot).unwrap_or(started);
        let Some(_worker) = self.workers.wait(peer.uid, until) else {
            tracing::debug!("the wait for a worker reached the time limit");
            return Response::Refused(Refusal::Busy);
        };

        // A snapshot that starts with less than half its time left would
        // probably finish after the limit. The helper would then discard it
        // and send `TimedOut`, after keeping the worker from other requests.
        if started.elapsed() > self.limits.snapshot / 2 {
            tracing::debug!("a worker became free too late for the snapshot");
            return Response::Refused(Refusal::Busy);
        }
        let snapshot = self
            .source
            .snapshot(request)
            .and_then(|snapshot| caller.redact(snapshot, &self.source));

        match snapshot {
            Ok(_) if started.elapsed() > self.limits.snapshot => {
                tracing::warn!(
                    elapsed = ?started.elapsed(),
                    "the snapshot took longer than the time limit"
                );
                Response::Refused(Refusal::TimedOut)
            }
            Ok(snapshot) => Response::Snapshot(Box::new(snapshot)),
            Err(error) => {
                tracing::error!(error = %ErrorChain(&error), "cannot take a snapshot");
                Response::Refused(Refusal::Failed)
            }
        }
    }

    /// Writes `response` on `stream`, or a refusal if the response cannot be
    /// sent, and returns what it wrote.
    fn respond(&self, stream: &UnixStream, response: Response) -> Result<Response, ServeError> {
        // The response has its own time limit, so that the helper can still
        // refuse a request that used up its time limit.
        let mut connection = Deadline::new(stream, self.limits.response);
        let refusal = match connection.write_message(&response) {
            Ok(()) => return Ok(response),
            Err(error @ ProtocolError::TooLarge { .. }) => {
                tracing::warn!(error = %ErrorChain(&error), "cannot send the snapshot");
                Refusal::ResponseTooLarge
            }
            Err(error @ ProtocolError::Encoding(_)) => {
                tracing::error!(error = %ErrorChain(&error), "cannot send the snapshot");
                Refusal::Failed
            }
            Err(error) => return Err(error.into()),
        };
        let response = Response::Refused(refusal);
        connection.write_message(&response)?;

        Ok(response)
    }
}

/// The refusal for a request that the helper could not read, or the error if
/// the connection itself failed. A client causes each of these faults, so the
/// helper logs them at the debug level.
fn refusal(error: ProtocolError) -> Result<Refusal, ProtocolError> {
    let refusal = match &error {
        ProtocolError::Version { .. } => Refusal::Version,
        ProtocolError::TooLarge { .. } => Refusal::RequestTooLarge,
        ProtocolError::Encoding(_) | ProtocolError::TrailingBytes(_) => Refusal::Malformed,
        ProtocolError::Io(io) if io.kind() == io::ErrorKind::TimedOut => Refusal::TimedOut,
        ProtocolError::Io(_) => {
            tracing::debug!(error = %ErrorChain(&error), "cannot read the request");
            return Err(error);
        }
    };

    tracing::debug!(error = %ErrorChain(&error), ?refusal, "cannot read the request");

    Ok(refusal)
}
