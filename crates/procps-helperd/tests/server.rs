// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Serves one connection: reads a request, takes a snapshot for the caller,
//! and sends the response.

use std::{
    ffi::OsString,
    io::{self, Write},
    num::NonZeroUsize,
    os::unix::net::UnixStream,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, SystemTime},
};

use assert_matches::assert_matches;
use darwin_proc::{Pid, Uid};
use pretty_assertions::assert_eq;
use procps_core::{
    Field, FieldGroup, FixtureSource, Process, ProcessSource, Snapshot, SnapshotRequest,
    SourceError,
    helper::{ProtocolError, ReadMessage, Refusal, Request, Response, VERSION, WriteMessage},
};
use procps_helperd::{ServeError, Server, TimeLimits};
use rstest::rstest;

/// How long a test client waits for a response before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// Time limits of `milliseconds` for each part of a connection.
const fn limits(milliseconds: u64) -> TimeLimits {
    let limit = Duration::from_millis(milliseconds);

    TimeLimits {
        request: limit,
        snapshot: limit,
        response: limit,
    }
}

/// An empty fixture.
fn empty() -> FixtureSource {
    FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new())
}

/// Sends `request` to `server` over a socket pair and returns the response.
fn exchange<S: ProcessSource + Sync>(
    server: &Server<S>,
    request: &[u8],
) -> Result<Response, Box<dyn std::error::Error>> {
    let (mut client, theirs) = UnixStream::pair()?;
    client.set_read_timeout(Some(PATIENCE))?;

    thread::scope(|scope| {
        let serving = scope.spawn(|| server.serve(&theirs));
        client.write_all(request)?;
        let response = client.read_message::<Response>()?;
        serving.join().map_err(|_| "the server thread panicked")??;

        Ok(response)
    })
}

/// The frame of `message`.
fn frame(message: &Request) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut wire = Vec::new();
    wire.write_message(message)?;

    Ok(wire)
}

/// The current process's record, with the process ID `pid`, the real,
/// effective and saved user ID `uid`, and an environment.
fn process(pid: i32, uid: u32) -> Result<Process, darwin_proc::Error> {
    let mut info = Pid::current().info()?;
    info.pid = Pid::from(pid);
    info.credentials.ruid = Uid::from(uid);
    info.credentials.euid = Uid::from(uid);
    info.credentials.svuid = Uid::from(uid);

    Ok(Process {
        environment: Some(Field::Available(vec![OsString::from("HOME=/")])),
        ..Process::from_identity(info, Field::Unsupported, Field::Unsupported)
    })
}

#[test]
fn a_snapshot_withholds_other_users_private_values() -> Result<(), Box<dyn std::error::Error>> {
    let me = Pid::current().info()?.credentials.euid.as_raw();
    let mine = process(10, me)?;
    let theirs = process(20, me + 1)?;
    let taken = SystemTime::UNIX_EPOCH;
    let uptime = Duration::from_secs(1);
    let server = Server::new(
        FixtureSource::new(taken, uptime, vec![mine.clone(), theirs.clone()]),
        TimeLimits::default(),
    );
    let request = SnapshotRequest::default().with(FieldGroup::Environment);

    assert_eq!(
        exchange(&server, &frame(&Request::Snapshot(request))?)?,
        Response::Snapshot(Box::new(Snapshot {
            taken,
            uptime,
            processes: vec![
                mine,
                Process {
                    environment: Some(Field::Denied),
                    ..theirs
                },
            ],
            system: None,
        }))
    );

    Ok(())
}

/// A message header with the protocol version `version` and the body length
/// `length`.
fn header(version: u16, length: u32) -> Vec<u8> {
    [&version.to_be_bytes()[..], &length.to_be_bytes()].concat()
}

#[rstest]
#[case::other_version(header(VERSION + 1, 0), Refusal::Version)]
#[case::too_large(header(VERSION, u32::MAX), Refusal::RequestTooLarge)]
#[case::malformed([header(VERSION, 1), vec![0x7f]].concat(), Refusal::Malformed)]
#[case::trailing_bytes([header(VERSION, 5), vec![0; 5]].concat(), Refusal::Malformed)]
// Half a header: the server waits for the rest until the time limit.
#[case::incomplete(header(VERSION, 0)[..3].to_vec(), Refusal::TimedOut)]
fn a_bad_request_is_refused(
    #[case] request: Vec<u8>,
    #[case] refusal: Refusal,
) -> Result<(), Box<dyn std::error::Error>> {
    let server = Server::new(empty(), limits(50));

    assert_eq!(exchange(&server, &request)?, Response::Refused(refusal));

    Ok(())
}

/// A source that waits for `delay`, and then fails if `broken` is set and
/// otherwise returns an empty snapshot.
struct Troubled {
    delay: Duration,
    broken: bool,
}

impl ProcessSource for Troubled {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        thread::sleep(self.delay);

        if self.broken {
            return Err(SourceError::ProcessTable(darwin_proc::Error::Os {
                call: darwin_proc::Call::ProcessTable,
                source: io::Error::from(io::ErrorKind::Other),
            }));
        }

        empty().snapshot(request)
    }
}

#[rstest]
#[case::slow(Troubled { delay: Duration::from_millis(100), broken: false }, Refusal::TimedOut)]
#[case::broken(Troubled { delay: Duration::ZERO, broken: true }, Refusal::Failed)]
fn a_snapshot_that_fails_is_refused(
    #[case] source: Troubled,
    #[case] refusal: Refusal,
) -> Result<(), Box<dyn std::error::Error>> {
    let server = Server::new(source, limits(50));
    let request = Request::Snapshot(SnapshotRequest::default());

    assert_eq!(
        exchange(&server, &frame(&request)?)?,
        Response::Refused(refusal)
    );

    Ok(())
}

#[test]
fn a_client_that_does_not_read_the_response_is_dropped() -> Result<(), Box<dyn std::error::Error>> {
    // Enough processes that the response fills the socket's buffer.
    let processes = (1..5000)
        .map(|pid| process(pid, 0))
        .collect::<Result<Vec<_>, _>>()?;
    let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, processes);
    let limits = TimeLimits {
        response: Duration::from_millis(100),
        ..TimeLimits::default()
    };
    let server = Server::new(source, limits);
    let (mut client, theirs) = UnixStream::pair()?;
    client.write_message(&Request::Snapshot(SnapshotRequest::default()))?;

    let result = server.serve(&theirs);

    assert_matches!(
        result,
        Err(ServeError::Protocol(ProtocolError::Io(error))) if error.kind() == io::ErrorKind::TimedOut
    );

    Ok(())
}

#[test]
fn a_client_that_closes_without_a_request_is_dropped() -> Result<(), Box<dyn std::error::Error>> {
    let (client, theirs) = UnixStream::pair()?;
    drop(client);

    assert_matches!(
        Server::new(empty(), TimeLimits::default()).serve(&theirs),
        Err(ServeError::Peer(darwin_proc::Error::Os {
            call: darwin_proc::Call::PeerProcess,
            ..
        }))
    );

    Ok(())
}

/// A source that sends a message on `started` when a snapshot of every
/// process starts, and takes `delay` to finish that snapshot. It answers a
/// request for selected processes at once, such as the second reading in
/// `Caller::redact`.
struct Reporting {
    started: Mutex<mpsc::Sender<()>>,
    delay: Duration,
}

impl ProcessSource for Reporting {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        if request.selected().is_none() {
            let _ = self.started.lock().map(|started| started.send(()));
            thread::sleep(self.delay);
        }

        empty().snapshot(request)
    }
}

#[rstest]
// The first snapshot keeps the only worker beyond the second request's limit.
#[case::past_the_limit(Duration::from_secs(3), Response::Refused(Refusal::TimedOut))]
// The first snapshot frees the worker when more than half of the second
// request's limit has passed.
#[case::past_half_the_limit(
    Duration::from_millis(1400),
    Response::Snapshot(Box::new(empty_snapshot()))
)]
fn a_request_that_waits_too_long_for_a_worker_is_refused(
    #[case] delay: Duration,
    #[case] first_response: Response,
) -> Result<(), Box<dyn std::error::Error>> {
    let (started, first_started) = mpsc::channel();
    let source = Reporting {
        started: Mutex::new(started),
        delay,
    };
    let limits = TimeLimits {
        snapshot: Duration::from_secs(2),
        ..TimeLimits::default()
    };
    let server = Server::new(source, limits).with_workers(NonZeroUsize::MIN);
    let request = frame(&Request::Snapshot(SnapshotRequest::default()))?;

    let responses = thread::scope(|scope| -> Result<_, Box<dyn std::error::Error>> {
        let first = scope.spawn(|| exchange(&server, &request).map_err(|error| error.to_string()));
        // The second request waits for the only worker, which the first
        // request's snapshot keeps.
        first_started.recv_timeout(PATIENCE)?;
        let second = exchange(&server, &request)?;
        let first = first.join().map_err(|_| "the first client panicked")??;

        Ok((first, second))
    })?;

    assert_eq!(
        responses,
        (first_response, Response::Refused(Refusal::Busy))
    );

    Ok(())
}

/// The snapshot of an empty fixture.
fn empty_snapshot() -> Snapshot {
    Snapshot {
        taken: SystemTime::UNIX_EPOCH,
        uptime: Duration::ZERO,
        processes: Vec::new(),
        system: None,
    }
}
