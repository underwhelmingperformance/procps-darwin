// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Accepts connections on a listening socket, and stops when the idle time has
//! passed since the listener started or since its last connection closed, but
//! not while a snapshot is running.

use std::{
    io::Write,
    num::NonZeroUsize,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime},
};

use assert_matches::assert_matches;
use pretty_assertions::assert_eq;
use procps_core::{
    FixtureSource, ProcessSource, Snapshot, SnapshotRequest, SourceError,
    helper::{ReadMessage, Refusal, Request, Response, VERSION, WriteMessage},
};
use procps_helperd::{ConnectionLimits, ListenError, Listener, Server, TimeLimits};
use rstest::rstest;

/// How long a test client waits for a response before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// The snapshot of an empty fixture.
fn empty_snapshot() -> Snapshot {
    Snapshot {
        taken: SystemTime::UNIX_EPOCH,
        uptime: Duration::ZERO,
        processes: Vec::new(),
        system: None,
    }
}

/// A listener for an empty fixture, with `request` to receive each request,
/// `idle` before it stops, and at most `total` connections, `per_user` of
/// them from one user.
fn listener(
    request: Duration,
    idle: Duration,
    total: usize,
    per_user: usize,
) -> Listener<FixtureSource> {
    let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
    let limits = TimeLimits {
        request,
        ..TimeLimits::default()
    };
    let connections = ConnectionLimits {
        total: NonZeroUsize::new(total).unwrap_or(NonZeroUsize::MIN),
        per_user: NonZeroUsize::new(per_user).unwrap_or(NonZeroUsize::MIN),
    };

    Listener::new(Server::new(source, limits), idle, connections)
}

/// A client connected to the socket at `path`.
fn connect(path: &Path) -> Result<UnixStream, Box<dyn std::error::Error>> {
    let client = UnixStream::connect(path)?;
    client.set_read_timeout(Some(PATIENCE))?;

    Ok(client)
}

#[test]
fn every_client_gets_a_response() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let listener = listener(Duration::from_secs(5), Duration::from_millis(200), 256, 64);

    let responses = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        let mut clients = [connect(&path)?, connect(&path)?];

        for client in &mut clients {
            client.write_message(&Request::Snapshot(SnapshotRequest::default()))?;
        }

        let responses = clients
            .iter_mut()
            .map(ReadMessage::read_message::<Response>)
            .collect::<Result<Vec<_>, _>>()?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok(responses)
    })?;

    assert_eq!(
        responses,
        vec![Response::Snapshot(Box::new(empty_snapshot())); 2]
    );

    Ok(())
}

#[test]
fn a_slow_client_keeps_the_listener_running() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let idle = Duration::from_millis(50);
    let listener = listener(Duration::from_secs(1), idle, 256, 64);
    // One byte of the header: the request takes its whole time limit, which
    // is longer than the idle time.
    let mut client = connect(&path)?;
    client.write_all(&VERSION.to_be_bytes()[..1])?;

    let outcome = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        thread::sleep(idle * 4);
        let running_while_served = !running.is_finished();
        let response = client.read_message::<Response>()?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok((running_while_served, response))
    })?;

    assert_eq!(outcome, (true, Response::Refused(Refusal::TimedOut)));

    Ok(())
}

#[test]
fn the_listener_stops_after_its_lifetime_when_no_connection_is_open()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let lifetime = Duration::from_millis(50);
    let listener =
        listener(Duration::from_secs(3), Duration::from_secs(60), 256, 64).lifetime(lifetime);
    // Send one byte of the header, so the slow request uses its whole 3-second
    // time limit.
    let mut slow = connect(&path)?;
    slow.write_all(&VERSION.to_be_bytes()[..1])?;
    let started = Instant::now();

    let outcome = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        // Connect after the lifetime has passed, while the slow request keeps
        // the listener running, so the late request shows that the listener
        // still accepts connections.
        thread::sleep(Duration::from_millis(1500));
        let mut late = connect(&path)?;
        late.write_message(&Request::Snapshot(SnapshotRequest::default()))?;
        let late_response = late.read_message::<Response>()?;
        let running_while_served = !running.is_finished();
        let slow_response = slow.read_message::<Response>()?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok((late_response, running_while_served, slow_response))
    })?;

    // The idle time is a minute, so only the lifetime can stop the listener
    // this soon.
    assert_eq!(
        (outcome, started.elapsed() < Duration::from_secs(10)),
        (
            (
                Response::Snapshot(Box::new(empty_snapshot())),
                true,
                Response::Refused(Refusal::TimedOut)
            ),
            true
        )
    );

    Ok(())
}

#[test]
fn a_listener_past_its_lifetime_leaves_a_new_connection_for_the_next_helper()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let lifetime = Duration::from_millis(50);
    let listener =
        listener(Duration::from_secs(1), Duration::from_secs(60), 256, 64).lifetime(lifetime);

    thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        thread::sleep(lifetime * 2);
        let mut client = connect(&path)?;
        client.write_message(&Request::Snapshot(SnapshotRequest::default()))?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok(())
    })?;

    // The connection is still waiting, as it would for launchd to start the
    // next helper.
    socket.set_nonblocking(true)?;
    let waiting = socket.accept().map(|_| ()).map_err(|error| error.kind());

    assert_eq!(waiting, Ok(()));

    Ok(())
}

/// A source that takes `delay` to answer, as a slow read from the kernel does.
struct Slow {
    delay: Duration,
}

impl ProcessSource for Slow {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        thread::sleep(self.delay);

        FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new()).snapshot(request)
    }
}

#[test]
fn the_listener_waits_for_a_snapshot_that_outlives_its_request()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let delay = Duration::from_secs(1);
    let limits = TimeLimits {
        snapshot: Duration::from_millis(100),
        ..TimeLimits::default()
    };
    let listener = Listener::new(
        Server::new(Slow { delay }, limits),
        Duration::from_millis(50),
        ConnectionLimits::default(),
    );
    let started = Instant::now();

    let response = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        let mut client = connect(&path)?;
        client.write_message(&Request::Snapshot(SnapshotRequest::default()))?;
        let response = client.read_message::<Response>()?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok(response)
    })?;

    // The listener returns only after the snapshot has finished.
    assert_eq!(
        (response, started.elapsed() >= delay),
        (Response::Refused(Refusal::TimedOut), true)
    );

    Ok(())
}

#[rstest]
#[case::per_user(256, 1)]
#[case::total(1, 64)]
fn a_connection_beyond_a_limit_is_refused(
    #[case] total: usize,
    #[case] per_user: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let listener = listener(
        Duration::from_millis(500),
        Duration::from_millis(50),
        total,
        per_user,
    );
    // The first client sends one byte of the header, so it keeps its
    // connection until the request's time limit.
    let mut first = connect(&path)?;
    first.write_all(&VERSION.to_be_bytes()[..1])?;
    let mut second = connect(&path)?;

    let responses = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let running = scope.spawn(|| listener.run(&socket));
        let second = second.read_message::<Response>()?;
        let first = first.read_message::<Response>()?;
        running
            .join()
            .map_err(|_| "the listener thread panicked")??;

        Ok((first, second))
    })?;

    assert_eq!(
        responses,
        (
            Response::Refused(Refusal::TimedOut),
            Response::Refused(Refusal::Busy)
        )
    );

    Ok(())
}

/// A source whose snapshots take 10 seconds, longer than any test waits.
struct Stuck;

impl ProcessSource for Stuck {
    fn snapshot(&self, _request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        thread::sleep(PATIENCE);

        Ok(empty_snapshot())
    }
}

#[test]
fn a_stuck_connection_stops_the_listener() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let socket = UnixListener::bind(&path)?;
    let listener = Listener::new(
        Server::new(Stuck, TimeLimits::default()),
        Duration::from_millis(50),
        ConnectionLimits::default(),
    )
    .stuck_after(Duration::from_millis(200));
    let mut client = connect(&path)?;
    client.write_message(&Request::Snapshot(SnapshotRequest::default()))?;

    assert_matches!(listener.run(&socket), Err(ListenError::Stuck(_)));

    Ok(())
}
