// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks how `pgrep` and `pkill` handle failed writes to standard output and
//! standard error. The reference harness always connects both streams to
//! pipes, so it cannot make a write fail.
//!
//! The tests either open the streams for reading only, so that each write fails
//! with `EBADF`, or close them. A write to the lowest closed standard stream
//! fails with `EINVAL` instead, as a write to procps-ng's epoll descriptor
//! does. A write to a pipe whose read end is closed raises `SIGPIPE`, which
//! ends the tool unless its parent ignores the signal.

use std::{
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    sync::{Mutex, MutexGuard, PoisonError},
};

use pretty_assertions::assert_eq;
use rstest::rstest;

/// Each test locks this mutex while it runs. On macOS, std creates a pipe and
/// marks its ends close-on-exec in a separate call. A child that another test
/// starts between the two calls keeps the pipe's read end open, so after the
/// test closes its read end, writes to the pipe still succeed.
static ALONE: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    ALONE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `tool` with `arguments` through `sh`, with `redirection` applied, and
/// returns its exit status and standard error.
fn run(tool: &str, arguments: &str, redirection: &str) -> std::io::Result<(Option<i32>, String)> {
    let script = format!("exec \"$0\" {arguments} {redirection}");
    let output = Command::new("/bin/sh")
        .args(["-c", &script, tool])
        .output()?;

    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

#[rstest]
#[case::pgrep(env!("CARGO_BIN_EXE_pgrep"), "pgrep")]
#[case::pkill(env!("CARGO_BIN_EXE_pkill"), "pkill")]
fn a_failed_write_to_standard_output_is_a_write_error(
    #[case] tool: &str,
    #[case] name: &str,
) -> std::io::Result<()> {
    let _alone = alone();
    assert_eq!(
        run(tool, "-h", "1</dev/null")?,
        (
            Some(1),
            format!("{name}: write error: Bad file descriptor\n")
        )
    );

    Ok(())
}

#[test]
fn a_failed_write_to_standard_error_makes_the_status_1() -> std::io::Result<()> {
    let _alone = alone();
    assert_eq!(
        run(env!("CARGO_BIN_EXE_pgrep"), "-z", "2</dev/null")?,
        (Some(1), String::new())
    );

    Ok(())
}

#[rstest]
#[case::pgrep(env!("CARGO_BIN_EXE_pgrep"))]
#[case::pkill(env!("CARGO_BIN_EXE_pkill"))]
fn a_write_to_a_closed_pipe_ends_the_tool_with_sigpipe(#[case] tool: &str) -> std::io::Result<()> {
    let _alone = alone();
    let (reader, writer) = std::io::pipe()?;
    drop(reader);

    let status = Command::new(tool)
        .arg("-h")
        .stdout(writer)
        .stderr(Stdio::null())
        .status()?;

    assert_eq!(
        (status.code(), status.signal()),
        (None, Some(libc::SIGPIPE))
    );

    Ok(())
}

// procps-ng's `main` calls `epoll_create` before anything else, so the epoll
// descriptor takes the lowest standard descriptor that was closed, and writes
// to it fail with `EINVAL`. Linux refuses to open the epoll descriptor through
// `/dev/stdin`, and the other closed descriptors stay closed.
#[rstest]
#[case::standard_output(env!("CARGO_BIN_EXE_pgrep"), "-h", ">&-", Some(1), "pgrep: write error: Invalid argument\n")]
#[case::standard_input_and_output(env!("CARGO_BIN_EXE_pgrep"), "-h", "<&- >&-", Some(1), "pgrep: write error: Bad file descriptor\n")]
#[case::pkill_standard_output(env!("CARGO_BIN_EXE_pkill"), "-h", ">&-", Some(1), "pkill: write error: Invalid argument\n")]
#[case::standard_error(env!("CARGO_BIN_EXE_pgrep"), "-z", "2>&-", Some(1), "")]
#[case::pid_file_on_standard_input(env!("CARGO_BIN_EXE_pgrep"), "-F /dev/stdin", "<&-", Some(1), "pgrep: Unable to open pidfile: No such device or address\n")]
#[case::pid_file_on_standard_output(env!("CARGO_BIN_EXE_pgrep"), "-F /dev/fd/1", "<&- >&-", Some(1), "pgrep: Unable to open pidfile: No such file or directory\n")]
#[case::pid_file_on_the_first_closed_descriptor(env!("CARGO_BIN_EXE_pgrep"), "-F /dev/fd/3", "3>&-", Some(1), "pgrep: Unable to open pidfile: No such device or address\n")]
#[case::pid_file_with_a_leading_zero(env!("CARGO_BIN_EXE_pgrep"), "-F /dev/fd/03", "3>&-", Some(1), "pgrep: Unable to open pidfile: No such file or directory\n")]
#[case::pid_file_on_a_later_closed_descriptor(env!("CARGO_BIN_EXE_pgrep"), "-F /dev/fd/4", "3>&- 4>&-", Some(1), "pgrep: Unable to open pidfile: No such file or directory\n")]
fn a_stream_that_is_closed_at_start_up_stays_closed(
    #[case] tool: &str,
    #[case] arguments: &str,
    #[case] redirection: &str,
    #[case] status: Option<i32>,
    #[case] stderr: &str,
) -> std::io::Result<()> {
    let _alone = alone();
    assert_eq!(
        run(tool, arguments, redirection)?,
        (status, stderr.to_owned())
    );

    Ok(())
}

#[rstest]
#[case::pgrep(env!("CARGO_BIN_EXE_pgrep"))]
#[case::pkill(env!("CARGO_BIN_EXE_pkill"))]
fn an_ignored_sigpipe_stays_ignored(#[case] tool: &str) -> std::io::Result<()> {
    let _alone = alone();
    let (reader, writer) = std::io::pipe()?;
    drop(reader);

    let output = Command::new("/bin/sh")
        .args(["-c", "trap '' PIPE; exec \"$0\" -h", tool])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()?;

    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned()
        ),
        (Some(0), String::new())
    );

    Ok(())
}
