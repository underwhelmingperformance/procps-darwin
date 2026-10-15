// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads the working directory and open file descriptors of live processes.

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
};

use assert_matches::assert_matches;
use darwin_proc::{Error, Pid, Status};
use pretty_assertions::assert_eq;

/// A shell that runs in `directory`, opens `/dev/null` as descriptor 7, and
/// then waits until its standard input closes.
struct WaitingChild(Child);

impl WaitingChild {
    /// Starts the shell and returns once it has opened descriptor 7.
    fn start(directory: &Path) -> std::io::Result<Self> {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec 7</dev/null; echo; read line"])
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        if let Some(stdout) = child.stdout.take() {
            BufReader::new(stdout).read_line(&mut String::new())?;
        }

        Ok(Self(child))
    }

    fn pid(&self) -> Result<Pid, std::num::TryFromIntError> {
        Ok(Pid::from(i32::try_from(self.0.id())?))
    }
}

impl Drop for WaitingChild {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

#[test]
fn a_child_reports_its_working_directory_and_descriptors() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let child = WaitingChild::start(directory.path())?;
    let pid = child.pid()?;

    let descriptors = pid.file_descriptors()?;
    let working_directory = pid.working_directory()?;

    assert_eq!(
        (
            working_directory,
            descriptors.get(..3),
            descriptors.contains(&7)
        ),
        (directory.path().canonicalize()?, Some(&[0, 1, 2][..]), true)
    );

    Ok(())
}

#[test]
fn another_users_working_directory_and_descriptors_are_denied() {
    let launchd = Pid::from(1);

    assert_matches!(
        (launchd.working_directory(), launchd.file_descriptors()),
        (Err(Error::Denied { .. }), Err(Error::Denied { .. }))
    );
}

#[test]
fn a_zombie_has_no_working_directory_or_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    // Wait without reaping, so that the child stays a zombie.
    while pid.info()?.status != Status::Zombie {
        std::thread::yield_now();
    }

    let results = (pid.working_directory(), pid.file_descriptors());
    child.wait()?;

    assert_matches!(
        results,
        (
            Err(Error::Unsupported { .. }),
            Err(Error::Unsupported { .. })
        )
    );

    Ok(())
}

#[test]
fn an_exited_process_has_no_working_directory_or_descriptors()
-> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    assert_matches!(
        (pid.working_directory(), pid.file_descriptors()),
        (Err(Error::Exited { .. }), Err(Error::Exited { .. }))
    );

    Ok(())
}
