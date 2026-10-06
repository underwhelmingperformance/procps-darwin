// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads the identity of live processes: this test process, children that it
//! starts, `launchd` and `kernel_task`.

use std::{
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
};

use assert_matches::assert_matches;
use darwin_proc::{Error, Pid, ProcessInfo};
use pretty_assertions::assert_eq;

/// The fields of a process that a test can predict.
#[derive(Debug, Eq, PartialEq)]
struct Known {
    pid: Pid,
    ppid: Pid,
    ruid: u32,
    euid: u32,
    comm: String,
}

impl From<&ProcessInfo> for Known {
    fn from(info: &ProcessInfo) -> Self {
        Self {
            pid: info.pid,
            ppid: info.ppid,
            ruid: info.credentials.ruid,
            euid: info.credentials.euid,
            comm: info.comm.clone(),
        }
    }
}

fn this_process() -> Result<Known, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let name = executable
        .file_name()
        .ok_or("the executable has no file name")?
        .to_string_lossy();

    Ok(Known {
        pid: Pid::current(),
        ppid: Pid::from(i32::try_from(std::os::unix::process::parent_id())?),
        ruid: rustix::process::getuid().as_raw(),
        euid: rustix::process::geteuid().as_raw(),
        comm: name.chars().take(16).collect(),
    })
}

#[test]
fn every_process_includes_this_one() -> Result<(), Box<dyn std::error::Error>> {
    let all = ProcessInfo::all()?;
    let found = all
        .iter()
        .find(|info| info.pid == Pid::current())
        .ok_or("this process is not listed")?;

    assert_eq!(Known::from(found), this_process()?);

    Ok(())
}

#[test]
fn a_process_is_read_by_pid() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(Known::from(&Pid::current().info()?), this_process()?);

    Ok(())
}

#[test]
fn another_users_process_is_read() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        Known::from(&Pid::from(1).info()?),
        Known {
            pid: Pid::from(1),
            ppid: Pid::from(0),
            ruid: 0,
            euid: 0,
            comm: "launchd".to_owned(),
        }
    );

    Ok(())
}

/// A shell that ignores `SIGUSR1`, catches `SIGUSR2` and waits, in a process
/// group of its own.
struct Child(std::process::Child);

impl Child {
    fn start() -> std::io::Result<Self> {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "trap '' USR1; trap 'exit' USR2; echo ready; read line",
            ])
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut line = String::new();

        if let Some(stdout) = child.stdout.take() {
            BufReader::new(stdout).read_line(&mut line)?;
        }

        Ok(Self(child))
    }

    fn pid(&self) -> Result<Pid, std::num::TryFromIntError> {
        Ok(Pid::from(i32::try_from(self.0.id())?))
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

#[test]
fn a_child_reports_its_group_parent_and_signal_dispositions()
-> Result<(), Box<dyn std::error::Error>> {
    let child = Child::start()?;
    let pid = child.pid()?;
    let info = pid.info()?;

    assert_eq!(
        (
            info.ppid,
            info.pgid,
            info.ignored_signals.contains(libc::SIGUSR1),
            info.caught_signals.contains(libc::SIGUSR2),
            info.caught_signals.contains(libc::SIGUSR1),
        ),
        (Pid::current(), pid, true, true, false)
    );

    Ok(())
}

#[test]
fn a_process_that_has_exited_is_reported_as_exited() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    assert_matches!(pid.info(), Err(Error::Exited { pid: exited }) if exited == pid);

    Ok(())
}

#[test]
fn the_executable_path_of_this_process_is_read() -> Result<(), Box<dyn std::error::Error>> {
    let expected = std::env::current_exe()?.canonicalize()?;

    assert_eq!(Pid::current().executable_path()?.canonicalize()?, expected);

    Ok(())
}

#[test]
fn kernel_task_has_no_executable_path() {
    assert_matches!(
        Pid::from(0).executable_path(),
        Err(Error::Unsupported { pid, .. }) if pid == Pid::from(0)
    );
}

#[test]
fn the_session_of_this_process_is_read() -> Result<(), Box<dyn std::error::Error>> {
    let expected = rustix::process::getsid(None)?.as_raw_nonzero().get();

    assert_eq!(Pid::current().session()?, Pid::from(expected));

    Ok(())
}

#[test]
fn launchd_has_an_executable_path() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(Pid::from(1).executable_path()?, Path::new("/sbin/launchd"));

    Ok(())
}

#[test]
fn kernel_task_has_no_session() {
    assert_matches!(
        Pid::from(0).session(),
        Err(Error::Unsupported { pid, .. }) if pid == Pid::from(0)
    );
}

#[test]
fn a_zombie_has_no_session() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    // Wait without reaping, so that the child stays a zombie.
    while pid.info()?.status != darwin_proc::Status::Zombie {
        std::thread::yield_now();
    }

    let session = pid.session();
    child.wait()?;

    assert_matches!(session, Err(Error::Unsupported { pid: zombie, .. }) if zombie == pid);

    Ok(())
}
