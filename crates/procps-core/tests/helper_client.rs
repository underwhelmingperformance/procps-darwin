// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Takes snapshots through the helper, and chooses a source.

use std::{
    ffi::OsString,
    io::{self, Write},
    os::unix::{
        ffi::OsStringExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime},
};

use assert_matches::assert_matches;
use darwin_proc::{Pid, Uid};
use pretty_assertions::assert_eq;
use procps_core::{
    ChoiceError, HelperOrLocal, ProcessSource, Snapshot, SnapshotRequest, Source, SourceChoice,
    SourceError,
    helper::{
        HelperError, HelperSource, ProtocolError, ReadMessage, Refusal, Request, Response, VERSION,
        WriteMessage,
    },
};
use rstest::rstest;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

/// How long a test client waits for the fake helper.
const PATIENCE: Duration = Duration::from_millis(500);

/// The snapshot that the fake helper sends.
fn snapshot() -> Snapshot {
    Snapshot {
        taken: SystemTime::UNIX_EPOCH,
        uptime: Duration::from_secs(1),
        processes: Vec::new(),
        system: None,
    }
}

/// This process's effective user ID.
fn me() -> Result<Uid, darwin_proc::Error> {
    Ok(Pid::current().info()?.credentials.euid)
}

/// How the fake helper handles its one connection.
#[derive(Clone, Debug)]
enum Behaviour {
    /// Reads the request and sends the response.
    Respond(Response),
    /// Sends these bytes at once and closes the connection without reading,
    /// as the helper does when it has too many connections open, or when a
    /// request has another version or is too large.
    SendAtOnce(Vec<u8>),
    /// Reads the request and closes the connection.
    Close,
    /// Reads the request and sends these bytes.
    Send(Vec<u8>),
    /// Reads the request and keeps the connection open for longer than the
    /// client waits.
    Stall,
}

/// Serves one connection on a socket at `path` as `behaviour` says. The
/// thread's result is the request, if the fake helper read one.
fn fake_helper(
    path: &Path,
    behaviour: Behaviour,
) -> io::Result<thread::JoinHandle<Result<Option<Request>, String>>> {
    let socket = UnixListener::bind(path)?;

    Ok(thread::spawn(move || {
        let mut stream = accept(&socket)?;

        if let Behaviour::SendAtOnce(bytes) = behaviour {
            stream
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;

            return Ok(None);
        }

        let request = stream
            .read_message::<Request>()
            .map_err(|error| error.to_string())?;

        match behaviour {
            Behaviour::Respond(response) => stream
                .write_message(&response)
                .map_err(|error| error.to_string())?,
            Behaviour::Send(bytes) => stream
                .write_all(&bytes)
                .map_err(|error| error.to_string())?,
            Behaviour::Stall => thread::sleep(PATIENCE * 2),
            Behaviour::Close | Behaviour::SendAtOnce(_) => {}
        }

        Ok(Some(request))
    }))
}

/// Accepts one connection on `socket`, waiting at most four times the client's
/// patience. A test whose client fails before it connects then reports the
/// client's error when it joins the fake helper.
fn accept(socket: &UnixListener) -> Result<UnixStream, String> {
    let wait = Timespec::try_from(PATIENCE * 4).map_err(|error| error.to_string())?;
    let mut ready = [PollFd::new(socket, PollFlags::IN)];

    match poll(&mut ready, Some(&wait)) {
        Ok(0) => Err("the client did not connect in time".to_owned()),
        Ok(_) => Ok(socket.accept().map_err(|error| error.to_string())?.0),
        Err(error) => Err(error.to_string()),
    }
}

/// A client for the fake helper at `path`. It accepts a helper that runs as
/// this process's user.
fn client(path: &Path) -> Result<HelperSource, darwin_proc::Error> {
    Ok(HelperSource::new(path).owned_by(me()?).patience(PATIENCE))
}

#[test]
fn the_helper_sends_the_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let helper = fake_helper(
        &path,
        Behaviour::Respond(Response::Snapshot(Box::new(snapshot()))),
    )?;
    let request = SnapshotRequest::default().with_system();

    let taken = client(&path)?.snapshot(&request)?;
    let received = helper.join().map_err(|_| "the helper panicked")??;

    assert_eq!(
        (taken, received),
        (snapshot(), Some(Request::Snapshot(request)))
    );

    Ok(())
}

/// A [`HelperError`] that tests can compare.
#[derive(Debug, PartialEq)]
enum Failure {
    Connect(io::ErrorKind),
    Untrusted,
    Io(io::ErrorKind),
    Version(u16),
    TooLarge,
    Refused(Refusal),
    Other(String),
}

impl From<SourceError> for Failure {
    fn from(error: SourceError) -> Self {
        match error {
            SourceError::Helper(HelperError::Connect(error)) => Self::Connect(error.kind()),
            SourceError::Helper(HelperError::Untrusted { .. }) => Self::Untrusted,
            SourceError::Helper(HelperError::Protocol(ProtocolError::Io(error))) => {
                Self::Io(error.kind())
            }
            SourceError::Helper(HelperError::Protocol(ProtocolError::Version { peer })) => {
                Self::Version(peer)
            }
            SourceError::Helper(HelperError::Protocol(ProtocolError::TooLarge { .. })) => {
                Self::TooLarge
            }
            SourceError::Helper(HelperError::Refused(refusal)) => Self::Refused(refusal),
            other => Self::Other(other.to_string()),
        }
    }
}

/// A frame header with `version` and the body length `length`.
fn header(version: u16, length: u32) -> Vec<u8> {
    [&version.to_be_bytes()[..], &length.to_be_bytes()].concat()
}

/// A frame that contains `response`, with `version` in its header. The helper
/// sends its own version when it refuses a request with another version.
fn frame(version: u16, response: &Response) -> Result<Vec<u8>, ProtocolError> {
    let mut frame = Vec::new();
    frame.write_message(response)?;
    frame.splice(..2, version.to_be_bytes());

    Ok(frame)
}

/// A refusal of a request with another version, as a newer helper sends it.
fn newer_version_refusal() -> Vec<u8> {
    frame(VERSION + 1, &Response::Refused(Refusal::Version)).expect("a refusal encodes")
}

/// A refusal because the helper is busy.
fn busy() -> Vec<u8> {
    frame(VERSION, &Response::Refused(Refusal::Busy)).expect("a refusal encodes")
}

/// A request for so many processes that it fills the socket's buffer.
fn large_request() -> SnapshotRequest {
    SnapshotRequest::default().for_processes((1..20_000).map(Pid::from))
}

#[rstest]
#[case::refusal(
    Behaviour::Respond(Response::Refused(Refusal::Busy)),
    SnapshotRequest::default(),
    Failure::Refused(Refusal::Busy)
)]
#[case::refusal_at_once(
    Behaviour::SendAtOnce(busy()),
    SnapshotRequest::default(),
    Failure::Refused(Refusal::Busy)
)]
#[case::version_refusal(
    Behaviour::SendAtOnce(newer_version_refusal()),
    SnapshotRequest::default(),
    Failure::Version(VERSION + 1)
)]
#[case::refusal_during_a_large_request(
    Behaviour::SendAtOnce(busy()),
    large_request(),
    Failure::Refused(Refusal::Busy)
)]
#[case::version_refusal_during_a_large_request(
    Behaviour::SendAtOnce(newer_version_refusal()),
    large_request(),
    Failure::Version(VERSION + 1)
)]
#[case::closed(
    Behaviour::Close,
    SnapshotRequest::default(),
    Failure::Io(io::ErrorKind::UnexpectedEof)
)]
#[case::other_version(Behaviour::Send(header(VERSION + 1, 0)), SnapshotRequest::default(), Failure::Version(VERSION + 1))]
#[case::too_large(
    Behaviour::Send(header(VERSION, u32::MAX)),
    SnapshotRequest::default(),
    Failure::TooLarge
)]
#[case::stalled(
    Behaviour::Stall,
    SnapshotRequest::default(),
    Failure::Io(io::ErrorKind::TimedOut)
)]
fn a_failed_exchange_is_an_error(
    #[case] behaviour: Behaviour,
    #[case] request: SnapshotRequest,
    #[case] expected: Failure,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let helper = fake_helper(&path, behaviour)?;

    let failure = client(&path)?.snapshot(&request).err().map(Failure::from);
    helper.join().map_err(|_| "the helper panicked")?.ok();

    assert_eq!(failure, Some(expected));

    Ok(())
}

#[test]
fn a_refusal_sent_at_once_always_reaches_the_client() -> Result<(), Box<dyn std::error::Error>> {
    // Darwin fails the client's write in more than one way when the helper
    // closes first, and each way shows up only in some runs.
    let mut unexpected = Vec::new();

    for attempt in 0..500 {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("socket");
        let helper = fake_helper(&path, Behaviour::SendAtOnce(busy()))?;
        let request = if attempt % 2 == 0 {
            SnapshotRequest::default()
        } else {
            large_request()
        };

        let failure = client(&path)?.snapshot(&request).err().map(Failure::from);
        helper.join().map_err(|_| "the helper panicked")??;

        if failure != Some(Failure::Refused(Refusal::Busy)) {
            unexpected.push(failure);
        }
    }

    assert_eq!(unexpected, Vec::new());

    Ok(())
}

#[test]
fn a_socket_with_another_owner_is_not_trusted() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let _socket = UnixListener::bind(&path)?;
    let me = me()?;
    let stranger = Uid::from(me.as_raw() + 1);

    let result = HelperSource::new(&path)
        .owned_by(stranger)
        .snapshot(&SnapshotRequest::default());

    assert_matches!(
        result,
        Err(SourceError::Helper(HelperError::Owner { found, expected }))
            if (found, expected) == (me, stranger)
    );

    Ok(())
}

/// The kind of file at the helper's path.
#[derive(Clone, Copy, Debug)]
enum NotASocket {
    /// A symbolic link to a socket that a fake helper listens on.
    Link,
    /// An ordinary file.
    File,
}

#[rstest]
#[case::link(NotASocket::Link)]
#[case::file(NotASocket::File)]
fn a_path_that_is_not_a_socket_is_not_trusted(
    #[case] kind: NotASocket,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let target = directory.path().join("target");
    let path = directory.path().join("socket");
    let _socket = match kind {
        NotASocket::Link => {
            let socket = UnixListener::bind(&target)?;
            std::os::unix::fs::symlink(&target, &path)?;
            Some(socket)
        }
        NotASocket::File => {
            std::fs::write(&path, "")?;
            None
        }
    };

    let result = client(&path)?.snapshot(&SnapshotRequest::default());

    assert_matches!(
        result,
        Err(SourceError::Helper(HelperError::NotSocket { path: found })) if found == path
    );

    Ok(())
}

#[test]
fn a_socket_with_another_link_is_not_trusted() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let target = directory.path().join("target");
    let path = directory.path().join("socket");
    let _socket = UnixListener::bind(&target)?;
    std::fs::hard_link(&target, &path)?;

    let result = client(&path)?.snapshot(&SnapshotRequest::default());

    assert_matches!(
        result,
        Err(SourceError::Helper(HelperError::Links { path: found, links: 2 })) if found == path
    );

    Ok(())
}

#[test]
fn a_missing_socket_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;

    let failure = client(&directory.path().join("socket"))?
        .snapshot(&SnapshotRequest::default())
        .err()
        .map(Failure::from);

    assert_eq!(failure, Some(Failure::Connect(io::ErrorKind::NotFound)));

    Ok(())
}

/// The process IDs in a snapshot.
fn pids(snapshot: &Snapshot) -> Vec<Pid> {
    snapshot
        .processes
        .iter()
        .map(|process| process.info.pid)
        .collect()
}

#[rstest]
#[case::refusal(Behaviour::Respond(Response::Refused(Refusal::Busy)), true)]
#[case::closed(Behaviour::Close, true)]
#[case::other_version(Behaviour::Send(header(VERSION + 1, 0)), false)]
#[case::version_refusal(Behaviour::SendAtOnce(newer_version_refusal()), false)]
fn helper_or_local_reads_locally_when_the_helper_fails(
    #[case] behaviour: Behaviour,
    #[case] still_uses_helper: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let helper = fake_helper(&path, behaviour)?;
    let source = HelperOrLocal::new(client(&path)?);
    let request = SnapshotRequest::default().for_processes([Pid::current()]);

    let read = pids(&source.snapshot(&request)?);
    helper.join().map_err(|_| "the helper panicked")?.ok();

    assert_eq!(
        (read, source.uses_helper()),
        (vec![Pid::current()], still_uses_helper)
    );

    Ok(())
}

/// The state of the socket at the helper's path.
#[derive(Clone, Copy, Debug)]
enum Socket {
    /// There is no file at the path.
    Missing,
    /// The socket belongs to another user.
    Untrusted,
    /// The socket file exists, and connections to it are refused, as they
    /// are when its listen backlog is full.
    Refusing,
}

#[rstest]
#[case::missing(Socket::Missing, false)]
#[case::untrusted(Socket::Untrusted, false)]
#[case::refusing(Socket::Refusing, true)]
fn helper_or_local_stops_using_a_helper_only_when_it_would_fail_again(
    #[case] socket: Socket,
    #[case] still_uses_helper: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let _listener = match socket {
        Socket::Missing => None,
        Socket::Untrusted => Some(UnixListener::bind(&path)?),
        Socket::Refusing => {
            drop(UnixListener::bind(&path)?);
            None
        }
    };
    let owner = match socket {
        Socket::Untrusted => Uid::from(me()?.as_raw() + 1),
        Socket::Missing | Socket::Refusing => me()?,
    };
    let source = HelperOrLocal::new(HelperSource::new(&path).owned_by(owner));
    let request = SnapshotRequest::default().for_processes([Pid::current()]);

    let read = pids(&source.snapshot(&request)?);

    assert_eq!(
        (read, source.uses_helper()),
        (vec![Pid::current()], still_uses_helper)
    );

    Ok(())
}

#[test]
fn threads_can_share_a_source() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let source = Source::HelperOrLocal(HelperOrLocal::new(client(
        &directory.path().join("socket"),
    )?));
    let request = SnapshotRequest::default().for_processes([Pid::current()]);

    let read = thread::scope(|scope| {
        let readers = [(); 2]
            .map(|()| scope.spawn(|| source.snapshot(&request).map(|snapshot| pids(&snapshot))));

        readers.map(|reader| reader.join().map_err(|_| "a reader panicked"))
    });

    assert_eq!(
        read.map(|pids| pids.map(Result::ok).ok().flatten()),
        [Some(vec![Pid::current()]), Some(vec![Pid::current()])]
    );

    Ok(())
}

#[test]
fn a_forced_helper_returns_its_error() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let source = Source::Helper(client(&directory.path().join("socket"))?);

    let failure = source
        .snapshot(&SnapshotRequest::default())
        .err()
        .map(Failure::from);

    assert_eq!(failure, Some(Failure::Connect(io::ErrorKind::NotFound)));

    Ok(())
}

/// The kind of source that a choice produces.
#[derive(Debug, PartialEq)]
enum Kind {
    Local,
    Helper,
    HelperOrLocal,
}

impl From<&Source> for Kind {
    fn from(source: &Source) -> Self {
        match source {
            Source::Local(_) => Self::Local,
            Source::Helper(_) => Self::Helper,
            Source::HelperOrLocal(_) => Self::HelperOrLocal,
        }
    }
}

#[rstest]
#[case::root(None, 0, true, Kind::Local)]
#[case::root_without_helper(None, 0, false, Kind::Local)]
#[case::helper_installed(None, 501, true, Kind::HelperOrLocal)]
#[case::helper_missing(None, 501, false, Kind::Local)]
#[case::empty_variable(Some(""), 501, true, Kind::HelperOrLocal)]
#[case::forced_local(Some("local"), 501, true, Kind::Local)]
#[case::forced_local_as_root(Some("local"), 0, true, Kind::Local)]
#[case::forced_helper(Some("helper"), 501, false, Kind::Helper)]
#[case::forced_helper_as_root(Some("helper"), 0, true, Kind::Helper)]
fn a_tool_chooses_its_source(
    #[case] variable: Option<&str>,
    #[case] euid: u32,
    #[case] installed: bool,
    #[case] expected: Kind,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("socket");

    if installed {
        std::fs::write(&socket, "")?;
    }

    let choice = SourceChoice {
        variable: variable.map(OsString::from),
        euid: Uid::from(euid),
        socket,
    };

    assert_eq!(Kind::from(&choice.choose()?), expected);

    Ok(())
}

#[rstest]
#[case::unknown(
    OsString::from("elsewhere"),
    r#"PROCPS_DARWIN_SOURCE is "elsewhere", but it must be "local", "helper" or empty"#
)]
#[case::not_utf8(
    OsString::from_vec(vec![0xff]),
    r#"PROCPS_DARWIN_SOURCE is "\xFF", but it must be "local", "helper" or empty"#
)]
fn an_unknown_source_is_an_error(#[case] value: OsString, #[case] message: &str) {
    let choice = SourceChoice {
        variable: Some(value.clone()),
        euid: Uid::from(501),
        socket: PathBuf::from("/nonexistent"),
    };
    let error = choice.choose();

    assert_matches!(
        error,
        Err(ref error @ ChoiceError::Unknown(ref found))
            if (found, error.to_string().as_str()) == (&value, message)
    );
}
