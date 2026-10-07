// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs the helper unprivileged on a temporary socket, to exercise the
//! transport.

use std::{
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
};

use darwin_proc::Pid;
use pretty_assertions::assert_eq;
use procps_core::{
    FieldGroup, SnapshotRequest,
    helper::{ReadMessage, Request, Response, WriteMessage},
};

#[test]
fn the_helper_serves_a_request_and_stops_when_idle() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    let mut helper = Command::new(env!("CARGO_BIN_EXE_procps-helperd"))
        .arg("--socket")
        .arg(&path)
        .args(["--idle-timeout", "1"])
        .env_remove("PROCPS_DARWIN_LOG")
        .stderr(Stdio::piped())
        .spawn()?;
    let mut log = BufReader::new(helper.stderr.take().ok_or("no standard error")?).lines();

    // The helper logs this event once it is listening.
    log.find(|line| line.as_ref().is_ok_and(|line| line.contains("listening")))
        .ok_or("the helper stopped before it listened")??;

    let mut client = UnixStream::connect(&path)?;
    let request = SnapshotRequest::default()
        .with(FieldGroup::Environment)
        .for_processes([Pid::current()]);
    client.write_message(&Request::Snapshot(request))?;
    let environments: Vec<_> = match client.read_message::<Response>()? {
        Response::Snapshot(snapshot) => snapshot
            .processes
            .into_iter()
            .map(|process| process.environment.map(|field| field.available().is_some()))
            .collect(),
        Response::Refused(refusal) => return Err(format!("refused: {refusal:?}").into()),
    };
    let rest: Vec<String> = log.collect::<Result<_, _>>()?;
    let status = helper.wait()?;
    let stopped = rest
        .iter()
        .any(|line| line.contains("stopping after the idle time"));
    // Any user can send requests as fast as the helper answers them, so the
    // default level must not log each one.
    let logged_the_request = rest.iter().any(|line| line.contains("sent a snapshot"));

    assert_eq!(
        (environments, status.success(), stopped, logged_the_request),
        (vec![Some(true)], true, true, false),
        "the helper logged {rest:?}"
    );

    Ok(())
}

#[test]
fn a_process_whose_executable_is_gone_is_not_logged() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("socket");
    // macOS kills a copy of a platform binary such as `sleep`, so the process
    // whose executable is deleted is another copy of the helper. It is also
    // killed if its executable is deleted before it has started, so delete the
    // executable once the copy is listening.
    let executable = directory.path().join("copy");
    std::fs::copy(env!("CARGO_BIN_EXE_procps-helperd"), &executable)?;
    let mut sleeper = Command::new(&executable)
        .arg("--socket")
        .arg(directory.path().join("other"))
        .args(["--idle-timeout", "60"])
        .stderr(Stdio::piped())
        .spawn()?;
    BufReader::new(sleeper.stderr.take().ok_or("no standard error")?)
        .lines()
        .find(|line| line.as_ref().is_ok_and(|line| line.contains("listening")))
        .ok_or("the copy stopped before it listened")??;
    std::fs::remove_file(&executable)?;
    let mut helper = Command::new(env!("CARGO_BIN_EXE_procps-helperd"))
        .arg("--socket")
        .arg(&path)
        .args(["--idle-timeout", "1"])
        .env_remove("PROCPS_DARWIN_LOG")
        .stderr(Stdio::piped())
        .spawn()?;
    let mut log = BufReader::new(helper.stderr.take().ok_or("no standard error")?).lines();
    log.find(|line| line.as_ref().is_ok_and(|line| line.contains("listening")))
        .ok_or("the helper stopped before it listened")??;

    // Reading the path of a deleted executable fails, and any user can start
    // such a process and ask for it as often as the helper answers.
    let pid = Pid::from(i32::try_from(sleeper.id())?);
    let mut client = UnixStream::connect(&path)?;
    client.write_message(&Request::Snapshot(
        SnapshotRequest::default().for_processes([pid]),
    ))?;
    let pids: Vec<_> = match client.read_message::<Response>()? {
        Response::Snapshot(snapshot) => snapshot
            .processes
            .into_iter()
            .map(|process| process.info.pid)
            .collect(),
        Response::Refused(refusal) => return Err(format!("refused: {refusal:?}").into()),
    };
    let still_running = sleeper.try_wait()?.is_none();
    let rest: Vec<String> = log.collect::<Result<_, _>>()?;
    helper.wait()?;
    sleeper.kill()?;
    sleeper.wait()?;
    let logged_the_failure = rest.iter().any(|line| line.contains("read failed"));

    assert_eq!(
        (pids, still_running, logged_the_failure),
        (vec![pid], true, false),
        "the helper logged {rest:?}"
    );

    Ok(())
}

#[test]
fn the_helper_needs_a_socket() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_procps-helperd"))
        .args(["--idle-timeout", "1"])
        .output()?;
    let logged = String::from_utf8(output.stderr)?;

    assert_eq!(
        (
            output.status.code(),
            logged.contains("cannot take the socket from launchd")
        ),
        (Some(1), true),
        "the helper logged {logged:?}"
    );

    Ok(())
}

#[test]
fn a_time_limit_must_be_at_least_a_second() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_procps-helperd"))
        .args(["--time-limit", "0"])
        .output()?;
    let message = String::from_utf8(output.stderr)?;

    assert_eq!(
        (
            output.status.code(),
            message.contains("0 is not a whole number of seconds from 1 to 3600")
        ),
        (Some(2), true),
        "the helper printed {message:?}"
    );

    Ok(())
}
