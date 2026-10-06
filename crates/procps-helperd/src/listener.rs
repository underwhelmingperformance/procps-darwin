// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashMap,
    io,
    num::NonZeroUsize,
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

use darwin_proc::{Peer, Uid};
use procps_core::ProcessSource;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

use crate::{ErrorChain, Server};

/// How often the listener checks for a connection that has run far beyond its
/// time limits, while it waits for the next connection. It must be less than a
/// second, because `run` puts the wait in the nanoseconds field of a
/// `Timespec`.
const WATCHDOG_INTERVAL: Duration = Duration::from_millis(999);

/// The shortest wait for a connection, so that a zero idle time does not make
/// the listener spin while it serves a connection.
const MINIMUM_WAIT: Duration = Duration::from_millis(10);

/// How long beyond its time limits a connection may run before the watchdog
/// stops the helper, unless [`Listener::stuck_after`] replaces the threshold.
const WATCHDOG_GRACE: Duration = Duration::from_secs(30);

/// How long the listener waits after a failed `accept`, such as when the
/// helper has run out of file descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The most connections that the helper keeps open at once.
///
/// These limits protect the helper from running out of file descriptors and
/// memory, and are far above what normal use reaches. The server's workers,
/// not these limits, decide how many snapshots run at once.
///
/// ```
/// use procps_helperd::ConnectionLimits;
///
/// let limits = ConnectionLimits::default();
///
/// assert_eq!((limits.total.get(), limits.per_user.get()), (256, 64));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionLimits {
    /// The most connections for all users together.
    pub total: NonZeroUsize,
    /// The most connections for one user, so that one user cannot take every
    /// connection.
    pub per_user: NonZeroUsize,
}

impl ConnectionLimits {
    /// The number of file descriptors that the helper needs for these limits:
    /// one for each connection, and 64 more for standard input, output and
    /// error, the listening socket, and descriptors that libraries open.
    ///
    /// ```
    /// use procps_helperd::ConnectionLimits;
    ///
    /// assert_eq!(ConnectionLimits::default().descriptors(), 320);
    /// ```
    #[must_use]
    pub fn descriptors(&self) -> u64 {
        u64::try_from(self.total.get())
            .unwrap_or(u64::MAX)
            .saturating_add(64)
    }
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            total: NonZeroUsize::MIN.saturating_add(255),
            per_user: NonZeroUsize::MIN.saturating_add(63),
        }
    }
}

/// Accepts connections and serves each one on its own thread. It returns when
/// the idle time has passed since it started or since its last connection
/// closed, whichever is later.
///
/// launchd starts the helper again when the next connection arrives, so the
/// helper does not need to keep running between uses.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use procps_core::FixtureSource;
/// use procps_helperd::{ConnectionLimits, Listener, Server, TimeLimits};
///
/// let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
/// let server = Server::new(source, TimeLimits::default());
/// let listener = Listener::new(server, Duration::from_secs(60), ConnectionLimits::default());
/// # let _ = listener;
/// ```
#[derive(Debug)]
pub struct Listener<S> {
    server: Arc<Server<S>>,
    idle: Duration,
    connections: ConnectionLimits,
    stuck: Duration,
}

/// An error that stops the helper from accepting connections.
#[derive(Debug, thiserror::Error)]
pub enum ListenError {
    /// Waiting for a connection failed.
    #[error("cannot wait for a connection")]
    Poll(#[from] rustix::io::Errno),
    /// A connection has run far beyond its time limits, probably because a
    /// read from the kernel is stuck. The helper stops, so that launchd starts
    /// a new helper for the next connection.
    #[error("a connection has run for {0:?}")]
    Stuck(Duration),
}

impl<S: ProcessSource + Send + Sync + 'static> Listener<S> {
    /// A listener that serves connections with `server`, returns when `idle`
    /// has passed since it started or since its last connection closed, and
    /// keeps at most `connections` open at once.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use procps_core::LocalSource;
    /// use procps_helperd::{ConnectionLimits, Listener, Server, TimeLimits};
    ///
    /// let server = Server::new(LocalSource, TimeLimits::default());
    /// let listener = Listener::new(server, Duration::from_secs(60), ConnectionLimits::default());
    /// # let _ = listener;
    /// ```
    #[must_use]
    pub fn new(server: Server<S>, idle: Duration, connections: ConnectionLimits) -> Self {
        let stuck = server.limits().total().saturating_add(WATCHDOG_GRACE);

        Self {
            server: Arc::new(server),
            idle,
            connections,
            stuck,
        }
    }

    /// This listener, stopping with [`ListenError::Stuck`] when a connection
    /// has run for `stuck`. The default is 30 seconds beyond the server's time
    /// limits.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use procps_core::LocalSource;
    /// use procps_helperd::{ConnectionLimits, Listener, Server, TimeLimits};
    ///
    /// let server = Server::new(LocalSource, TimeLimits::default());
    /// let listener = Listener::new(server, Duration::from_secs(60), ConnectionLimits::default())
    ///     .stuck_after(Duration::from_secs(120));
    /// # let _ = listener;
    /// ```
    #[must_use]
    pub fn stuck_after(self, stuck: Duration) -> Self {
        Self { stuck, ..self }
    }

    /// Serves the connections on `socket`, and returns when the idle time has
    /// passed since it started or since its last connection closed, whichever
    /// is later.
    ///
    /// A client that would exceed a connection limit gets
    /// [`Refusal::Busy`](procps_core::helper::Refusal::Busy).
    ///
    /// ```
    /// use std::{
    ///     os::unix::net::UnixListener,
    ///     time::{Duration, SystemTime},
    /// };
    ///
    /// use procps_core::FixtureSource;
    /// use procps_helperd::{ConnectionLimits, Listener, Server, TimeLimits};
    ///
    /// let directory = tempfile::tempdir()?;
    /// let socket = UnixListener::bind(directory.path().join("socket"))?;
    /// let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
    /// let server = Server::new(source, TimeLimits::default());
    ///
    /// Listener::new(
    ///     server,
    ///     Duration::from_millis(10),
    ///     ConnectionLimits::default(),
    /// )
    /// .run(&socket)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ListenError`] if waiting for a connection fails, or if a
    /// connection runs far beyond its time limits.
    pub fn run(&self, socket: &UnixListener) -> Result<(), ListenError> {
        let wait = self.idle.clamp(MINIMUM_WAIT, WATCHDOG_INTERVAL);
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: wait.subsec_nanos().into(),
        };
        let slots = Arc::new(Slots::new(self.connections));

        loop {
            let mut ready = [PollFd::new(socket, PollFlags::IN)];

            match poll(&mut ready, Some(&wait)) {
                Ok(0) => {}
                Ok(_) => self.accept(socket, &slots),
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }

            let state = slots.state();

            if let Some(oldest) = state.oldest() {
                if oldest > self.stuck {
                    tracing::error!(?oldest, "a connection is stuck, so stopping");
                    return Err(ListenError::Stuck(oldest));
                }
            } else if state.quiet.elapsed() >= self.idle {
                tracing::info!("stopping after the idle time");
                return Ok(());
            }
        }
    }

    /// Accepts one connection and serves it, or refuses it if it would
    /// exceed a limit.
    fn accept(&self, socket: &UnixListener, slots: &Arc<Slots>) {
        let stream = match socket.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return,
            Err(error) => {
                tracing::error!(error = %ErrorChain(&error), "cannot accept a connection");
                thread::sleep(ACCEPT_BACKOFF);
                return;
            }
        };

        let peer = match Peer::of(&stream) {
            Ok(peer) => peer,
            Err(error) => {
                tracing::debug!(error = %ErrorChain(&error), "cannot identify the client");
                return;
            }
        };

        let Some(slot) = Slots::take(slots, peer.uid) else {
            tracing::debug!(uid = peer.uid.as_raw(), "too many connections, so refusing");

            if let Err(error) = self.server.refuse_busy(&stream) {
                tracing::debug!(error = %ErrorChain(&error), "cannot refuse the connection");
            }

            return;
        };

        let server = Arc::clone(&self.server);
        let spawned = thread::Builder::new().spawn(move || {
            serve(&server, &stream, peer);
            drop(slot);
        });

        if let Err(error) = spawned {
            tracing::error!(error = %ErrorChain(&error), "cannot start a thread for a connection");
        }
    }
}

/// Serves `stream`, whose client is `peer`, with `server`, and logs an error
/// that ends the connection.
fn serve<S: ProcessSource>(server: &Server<S>, stream: &UnixStream, peer: Peer) {
    if let Err(error) = server.serve_peer(stream, peer) {
        tracing::debug!(error = %ErrorChain(&error), "cannot serve a connection");
    }
}

/// Counts the connections being served, and limits them.
#[derive(Debug)]
struct Slots {
    state: Mutex<State>,
    limits: ConnectionLimits,
}

/// The connections being served.
#[derive(Debug)]
struct State {
    /// When each connection started, by an identifier for the connection.
    started: HashMap<u64, (Uid, Instant)>,
    /// The identifier for the next connection.
    next: u64,
    /// When the last connection ended, or when the listener started.
    quiet: Instant,
}

impl State {
    /// How long the oldest connection has been served, if one is.
    fn oldest(&self) -> Option<Duration> {
        self.started
            .values()
            .map(|(_, started)| started.elapsed())
            .max()
    }
}

impl Slots {
    fn new(limits: ConnectionLimits) -> Self {
        Self {
            state: Mutex::new(State {
                started: HashMap::new(),
                next: 0,
                quiet: Instant::now(),
            }),
            limits,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes a slot for a connection from `uid`, or `None` if the connection
    /// would exceed a limit.
    fn take(slots: &Arc<Self>, uid: Uid) -> Option<Slot> {
        let mut state = slots.state();
        let from_user = state
            .started
            .values()
            .filter(|(user, _)| *user == uid)
            .count();

        if state.started.len() >= slots.limits.total.get()
            || from_user >= slots.limits.per_user.get()
        {
            return None;
        }

        let id = state.next;
        state.next += 1;
        state.started.insert(id, (uid, Instant::now()));

        Some(Slot {
            slots: Arc::clone(slots),
            id,
        })
    }
}

/// A slot taken for one connection. Dropping it frees the slot, even if the
/// thread that serves the connection panics.
struct Slot {
    slots: Arc<Slots>,
    id: u64,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut state = self.slots.state();
        state.started.remove(&self.id);

        if state.started.is_empty() {
            state.quiet = Instant::now();
        }
    }
}
