// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks that the handlers of `darwin_proc::catch_signals` respond to a
//! signal as they are told to. Each case runs in a child process, the test
//! binary itself, because a handler stays installed and ends the process.

use std::{
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
};

use assert_matches::assert_matches;
use darwin_proc::{Call, Pid, SignalResponse};
use pretty_assertions::assert_eq;
use rstest::rstest;

/// The variable that tells a child which case to run.
const CASE: &str = "DARWIN_PROC_CATCH_CASE";

/// The response of the cases that exit with status 3.
fn exit_3() -> SignalResponse {
    SignalResponse::Exit {
        message: b"caught\n".to_vec(),
        status: 3,
    }
}

/// The signal that each case sends, and the response that it installs for the
/// signal.
fn case(name: &str) -> Option<(libc::c_int, SignalResponse)> {
    let message = |text: &str| text.as_bytes().to_vec();

    match name {
        "exit" | "retry" | "busy" => Some((libc::SIGUSR1, exit_3())),
        "discarded" => Some((
            libc::SIGURG,
            SignalResponse::Default {
                message: message("discarded\n"),
            },
        )),
        "quiet-exit" => Some((
            libc::SIGPIPE,
            SignalResponse::Exit {
                message: Vec::new(),
                status: 0,
            },
        )),
        "default" => Some((
            libc::SIGTERM,
            SignalResponse::Default {
                message: message("dying\n"),
            },
        )),
        _ => None,
    }
}

/// Run by each child: installs the response of the case in `CASE`, then sends
/// the case's signal to this process. The handler ends the process, so the
/// test fails if the signal returns. The `retry` case first makes a call
/// fail, and the `busy` case makes a second call after the first.
#[test]
#[ignore = "the other tests run this test in a child process"]
fn child() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var(CASE)?;
    let (signal, response) = case(&name).ok_or("unknown case")?;
    let refused = |result: Result<(), darwin_proc::Error>, code| matches!(result, Err(darwin_proc::Error::Os { source, .. }) if source.raw_os_error() == Some(code));

    if name == "retry" {
        let failed = darwin_proc::catch_signals([(signal, exit_3()), (libc::SIGKILL, exit_3())]);

        if !refused(failed, libc::EINVAL) {
            return Err("SIGKILL was not refused".into());
        }
    }

    darwin_proc::catch_signals([(signal, response)])?;

    if name == "busy"
        && !refused(
            darwin_proc::catch_signals([(signal, exit_3())]),
            libc::EBUSY,
        )
    {
        return Err("the second call was not refused".into());
    }

    Pid::current().send_signal(signal)?;

    Err("the handler returned".into())
}

/// The exit status, the signal that ended the child, and the child's
/// standard error.
type Ending = (Option<i32>, Option<i32>, String);

fn run(name: &str) -> Result<Ending, Box<dyn std::error::Error>> {
    let (signal, _) = case(name).ok_or("unknown case")?;

    // The child inherits this thread's signal mask, which the process that
    // runs the tests may have set to block the signal.
    darwin_proc::unblock_signal(signal)?;

    let output = Command::new(std::env::current_exe()?)
        .args(["child", "--exact", "--ignored", "--quiet"])
        .env(CASE, name)
        .stdout(Stdio::null())
        .output()?;

    Ok((
        output.status.code(),
        output.status.signal(),
        String::from_utf8(output.stderr)?,
    ))
}

#[rstest]
#[case::exit("exit", (Some(3), None, "caught\n".to_owned()))]
#[case::quiet_exit("quiet-exit", (Some(0), None, String::new()))]
#[case::default_action("default", (None, Some(libc::SIGTERM), "dying\n".to_owned()))]
#[case::default_action_that_discards("discarded", (Some(1), None, "discarded\n".to_owned()))]
#[case::retry_after_a_refused_call("retry", (Some(3), None, "caught\n".to_owned()))]
#[case::second_call("busy", (Some(3), None, "caught\n".to_owned()))]
fn a_handler_responds_as_it_was_told(
    #[case] name: &str,
    #[case] expected: Ending,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(run(name)?, expected);

    Ok(())
}

#[rstest]
#[case::zero(0)]
#[case::beyond_darwin(32)]
#[case::kill(libc::SIGKILL)]
#[case::stop(libc::SIGSTOP)]
fn a_signal_that_no_handler_can_catch_is_an_error(#[case] signal: libc::c_int) {
    let response = SignalResponse::Default {
        message: Vec::new(),
    };

    assert_matches!(
        darwin_proc::catch_signals([(signal, response)]),
        Err(darwin_proc::Error::Os { call: Call::SignalAction, source })
            if source.raw_os_error() == Some(libc::EINVAL)
    );
}
