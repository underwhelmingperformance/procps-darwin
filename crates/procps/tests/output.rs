// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks how `pgrep`, `pkill` and `ps` handle failed writes to standard output
//! and standard error. The reference harness always connects both streams to
//! pipes, so it cannot make a write fail.
//!
//! The tests either open the streams for reading only, so that each write fails
//! with `EBADF`, or close them. For `pgrep` and `pkill`, a write to the lowest
//! closed standard stream fails with `EINVAL` instead, as a write to
//! procps-ng's epoll descriptor does. procps-ng's `ps` has no epoll descriptor.
//! A write to a pipe whose read end is closed raises `SIGPIPE`, which ends
//! `pgrep` and `pkill` unless their parent ignores the signal. `ps` catches the
//! signal and exits with status 0.

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

// procps-ng's `pgrep` calls `epoll_create` before anything else, so the epoll
// descriptor takes the lowest standard descriptor that was closed, and writes
// to it fail with `EINVAL`. Linux refuses to open the epoll descriptor through
// `/dev/stdin`, and the other closed descriptors stay closed. procps-ng's `ps`
// keeps no epoll descriptor open in a closed stream's place, so its writes to a
// closed stream fail with `EBADF`.
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
#[case::ps_standard_output(env!("CARGO_BIN_EXE_ps"), "--help", ">&-", Some(1), "ps: write error: Bad file descriptor\n")]
#[case::ps_standard_input_and_output(env!("CARGO_BIN_EXE_ps"), "--help", "<&- >&-", Some(1), "ps: write error: Bad file descriptor\n")]
#[case::ps_listing(env!("CARGO_BIN_EXE_ps"), "L", ">&-", Some(1), "ps: write error: Bad file descriptor\n")]
#[case::ps_standard_error(env!("CARGO_BIN_EXE_ps"), "-E", "2>&-", Some(1), "")]
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

/// A process whose command line is longer than a stream's buffer, so that
/// `pgrep -a` writes its line with one call that fails, and leaves nothing in
/// the buffer. Dropping it ends the process.
struct LongCommandLine(std::process::Child);

impl LongCommandLine {
    fn start() -> std::io::Result<Self> {
        let long = "y".repeat(5000);
        let child = Command::new("/bin/sh")
            .args(["-c", "read line", &long])
            .stdin(Stdio::piped())
            .spawn()?;

        Ok(Self(child))
    }
}

impl Drop for LongCommandLine {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

// With standard input and standard output closed, procps-ng's epoll
// descriptor takes descriptor 0, and descriptor 1 stays closed. `fclose`
// then fails with `EBADF` after the failed write. A pid file opened by its
// path takes descriptor 1, and `fclose` closes it without an error.
#[rstest]
#[case::closed("-p", "pgrep: write error: Bad file descriptor\n")]
#[case::pid_file("-F", "pgrep: write error\n")]
fn closing_standard_output_after_a_failed_write_is_reported_as_in_procps_ng(
    #[case] option: &str,
    #[case] stderr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let _alone = alone();
    let process = LongCommandLine::start()?;
    let directory = tempfile::tempdir()?;
    let pid_file = directory.path().join("long.pid");
    std::fs::write(&pid_file, format!("{}\n", process.0.id()))?;
    let value = if option == "-p" {
        process.0.id().to_string()
    } else {
        pid_file.to_str().ok_or("path is not UTF-8")?.to_owned()
    };

    assert_eq!(
        run(
            env!("CARGO_BIN_EXE_pgrep"),
            &format!("-a {option} {value}"),
            "<&- >&-"
        )?,
        (Some(1), stderr.to_owned())
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

// procps-ng's `ps` catches `SIGPIPE` and exits with status 0, even when its
// parent ignores the signal.
#[rstest]
#[case::default_disposition("exec \"$0\" --help all")]
#[case::ignored("trap '' PIPE; exec \"$0\" --help all")]
fn ps_exits_with_status_0_on_a_closed_pipe(#[case] script: &str) -> std::io::Result<()> {
    let _alone = alone();
    let (reader, writer) = std::io::pipe()?;
    drop(reader);

    let output = Command::new("/bin/sh")
        .args(["-c", script, env!("CARGO_BIN_EXE_ps")])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()?;

    assert_eq!(
        (
            output.status.code(),
            output.status.signal(),
            String::from_utf8_lossy(&output.stderr).into_owned()
        ),
        (Some(0), None, String::new())
    );

    Ok(())
}

// The kernel sends `SIGXFSZ` at once when a write passes the file size limit,
// so the test needs no timing. procps-ng's `ps` reports the signal and a bug,
// and then dies of it.
#[test]
fn ps_reports_a_signal_that_its_handler_catches() -> Result<(), Box<dyn std::error::Error>> {
    let _alone = alone();
    let tool = env!("CARGO_BIN_EXE_ps");
    let directory = tempfile::tempdir()?;

    // The child inherits this thread's signal mask. With `SIGXFSZ` blocked,
    // the write would fail with `EFBIG` and raise no signal.
    darwin_proc::unblock_signal(libc::SIGXFSZ)?;

    let output = Command::new("/bin/sh")
        .args([
            "-c",
            "ulimit -f 1; exec \"$0\" --help all > \"$1\"",
            tool,
            directory
                .path()
                .join("help")
                .to_str()
                .ok_or("path is not UTF-8")?,
        ])
        .stderr(Stdio::piped())
        .output()?;

    assert_eq!(
        (
            output.status.code(),
            output.status.signal(),
            String::from_utf8(output.stderr)?
        ),
        (
            None,
            Some(libc::SIGXFSZ),
            format!(
                "Signal 25 (XFSZ) caught by ps (4.0.7).\n{tool}:src/ps/display.c:75: please report this bug\n"
            )
        )
    );

    Ok(())
}
