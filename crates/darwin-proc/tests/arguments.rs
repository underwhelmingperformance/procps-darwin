// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads the arguments and environment of live processes.

use std::{
    ffi::OsString,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
};

use assert_matches::assert_matches;
use darwin_proc::{Arguments, Error, Pid, Status};
use pretty_assertions::assert_eq;

#[test]
fn this_process_reads_its_own_arguments_and_environment() -> Result<(), Box<dyn std::error::Error>>
{
    let mut arguments = Pid::current().arguments()?;
    let mut environment: Vec<OsString> = std::env::vars_os()
        .map(|(name, value)| {
            let mut entry = name;
            entry.push("=");
            entry.push(value);
            entry
        })
        .collect();
    arguments.environment.sort();
    environment.sort();

    assert_eq!(
        (arguments.arguments, arguments.environment),
        (std::env::args_os().collect::<Vec<_>>(), environment)
    );

    Ok(())
}

fn strings(strings: &[&str]) -> Vec<OsString> {
    strings.iter().map(OsString::from).collect()
}

/// Reads the arguments of `command` while it runs, then closes its standard
/// input and waits for it. `command` must run until its standard input
/// closes.
fn arguments_of(command: &mut Command) -> Result<Arguments, Box<dyn std::error::Error>> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let arguments = Pid::from(i32::try_from(child.id())?).arguments();
    drop(child.stdin.take());
    child.wait()?;

    Ok(arguments?)
}

/// Panics unless `actual` equals one of the values in `expected`.
///
/// macOS 27 withholds the environment of Apple's platform binaries from other
/// processes, and macOS 26, which CI uses, returns it, so some tests accept
/// either result.
fn assert_one_of(actual: &Arguments, expected: &[Arguments]) {
    assert!(
        expected.contains(actual),
        "{actual:#?} is none of {expected:#?}"
    );
}

#[test]
fn a_platform_binary_shows_its_arguments() -> Result<(), Box<dyn std::error::Error>> {
    // `cat` reads standard input (the `-` operand) until the test closes it,
    // so the child runs while the test reads its arguments.
    let arguments = arguments_of(
        Command::new("/bin/cat")
            .args(["-", "two words"])
            .env_clear()
            .env("VISIBLE", "no"),
    )?;

    let withheld = Arguments {
        executable: PathBuf::from("/bin/cat"),
        arguments: strings(&["/bin/cat", "-", "two words"]),
        environment: Vec::new(),
    };
    let returned = Arguments {
        environment: strings(&["VISIBLE=no"]),
        ..withheld.clone()
    };

    assert_one_of(&arguments, &[withheld, returned]);

    Ok(())
}

/// When macOS withholds a platform binary's environment, the kernel truncates
/// the buffer after `argc` strings, which it counts after skipping every NUL
/// that follows the path. With an empty `argv[0]`, the count includes the first
/// environment string, so `environment` contains only `FIRST=1`.
#[test]
fn an_empty_first_argument_is_kept() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = arguments_of(
        Command::new("/bin/cat")
            .arg0("")
            .args(["-", "two words"])
            .env_clear()
            .env("FIRST", "1")
            .env("SECOND", "2"),
    )?;

    let withheld = Arguments {
        executable: PathBuf::from("/bin/cat"),
        arguments: strings(&["", "-", "two words"]),
        environment: strings(&["FIRST=1"]),
    };
    let returned = Arguments {
        environment: strings(&["FIRST=1", "SECOND=2"]),
        ..withheld.clone()
    };

    assert_one_of(&arguments, &[withheld, returned]);

    Ok(())
}

/// The variable that makes [`helper_waits_until_its_input_closes`] wait.
const HELPER: &str = "DARWIN_PROC_ARGUMENTS_HELPER";

/// Runs as the child of
/// [`another_process_of_this_user_can_show_its_environment`]. This test binary
/// is not an Apple platform binary, so macOS returns its environment.
#[test]
#[ignore = "runs only as a child of another test"]
fn helper_waits_until_its_input_closes() -> std::io::Result<()> {
    if std::env::var_os(HELPER).is_some() {
        std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink())?;
    }

    Ok(())
}

#[test]
fn another_process_of_this_user_can_show_its_environment() -> Result<(), Box<dyn std::error::Error>>
{
    let executable = std::env::current_exe()?;
    let options = [
        "--ignored",
        "--exact",
        "helper_waits_until_its_input_closes",
    ];
    let arguments = arguments_of(
        Command::new(&executable)
            .args(options)
            .env_clear()
            .env(HELPER, "1"),
    )?;
    let mut expected = vec![executable.clone().into_os_string()];
    expected.extend(strings(&options));

    assert_eq!(
        arguments,
        Arguments {
            executable,
            arguments: expected,
            environment: strings(&["DARWIN_PROC_ARGUMENTS_HELPER=1"]),
        }
    );

    Ok(())
}

#[test]
fn another_users_arguments_are_denied() {
    assert_matches!(
        Pid::from(1).arguments(),
        Err(Error::Denied { pid, .. }) if pid == Pid::from(1)
    );
}

#[test]
fn kernel_task_has_no_arguments() {
    assert_matches!(
        Pid::from(0).arguments(),
        Err(Error::Unsupported { pid, .. }) if pid == Pid::from(0)
    );
}

#[test]
fn a_zombie_has_no_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    // Wait without reaping, so that the child stays a zombie.
    while pid.info()?.status != Status::Zombie {
        std::thread::yield_now();
    }

    let arguments = pid.arguments();
    child.wait()?;

    assert_matches!(arguments, Err(Error::Unsupported { pid: zombie, .. }) if zombie == pid);

    Ok(())
}

#[test]
fn an_exited_process_has_no_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    assert_matches!(pid.arguments(), Err(Error::Exited { pid: exited }) if exited == pid);

    Ok(())
}
