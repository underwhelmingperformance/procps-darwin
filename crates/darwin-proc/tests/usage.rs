// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads CPU, memory and thread data of live processes.

use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use assert_matches::assert_matches;
use darwin_proc::{Error, Pid, RunState, SchedulingPolicy, Status};
use pretty_assertions::assert_eq;

/// A single-threaded shell that uses some CPU time, then waits until its
/// standard input closes.
struct BusyChild(std::process::Child);

impl BusyChild {
    fn start() -> std::io::Result<Self> {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "i=0; while [ $i -lt 300000 ]; do i=$((i + 1)); done; echo done; read line",
            ])
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

impl Drop for BusyChild {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

fn close(a: Duration, b: Duration) -> bool {
    a.abs_diff(b) < Duration::from_millis(20)
}

#[test]
fn the_task_its_usage_and_its_thread_agree_on_cpu_time() -> Result<(), Box<dyn std::error::Error>> {
    let child = BusyChild::start()?;
    let pid = child.pid()?;

    // The shell prints `done` before it calls `read`, so poll until its thread
    // is in the waiting state.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while pid.threads()?.first().map(|thread| thread.run_state) != Some(RunState::Waiting) {
        if std::time::Instant::now() > deadline {
            return Err("the child never started to wait".into());
        }

        std::thread::yield_now();
    }

    let task = pid.task_info()?;
    let (usage, _) = pid.resource_usage()?;
    let threads = pid.threads()?;
    let [thread] = threads.as_slice() else {
        return Err(format!("expected one thread, found {threads:?}").into());
    };

    assert_eq!(
        (
            task.threads,
            close(task.user_time, thread.user_time),
            close(usage.user_time, thread.user_time),
            task.user_time > Duration::from_millis(50),
            thread.run_state,
            thread.policy,
        ),
        (
            1,
            true,
            true,
            true,
            RunState::Waiting,
            SchedulingPolicy::Timeshare,
        )
    );

    Ok(())
}

#[test]
fn a_named_thread_is_listed_by_name() -> Result<(), Box<dyn std::error::Error>> {
    let (sender, receiver) = mpsc::channel::<()>();
    let (ready_sender, ready) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("darwin-proc-test".to_owned())
        .spawn(move || {
            let _ = ready_sender.send(());
            let _ = receiver.recv();
        })?;
    ready.recv()?;

    let names: Vec<String> = Pid::current()
        .threads()?
        .into_iter()
        .map(|thread| thread.name)
        .filter(|name| name == "darwin-proc-test")
        .collect();
    drop(sender);
    thread.join().map_err(|_| "the thread panicked")?;

    assert_eq!(names, ["darwin-proc-test"]);

    Ok(())
}

#[test]
fn another_users_usage_is_denied() {
    let launchd = Pid::from(1);

    assert_matches!(
        (
            launchd.task_info(),
            launchd.resource_usage(),
            launchd.threads()
        ),
        (
            Err(Error::Denied { .. }),
            Err(Error::Denied { .. }),
            Err(Error::Denied { .. })
        )
    );
}

#[test]
fn a_zombie_has_no_usage() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    // Wait without reaping, so that the child stays a zombie.
    while pid.info()?.status != Status::Zombie {
        std::thread::yield_now();
    }

    let results = (pid.task_info(), pid.threads());
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
fn an_exited_process_has_no_usage() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    assert_matches!(
        (pid.task_info(), pid.resource_usage(), pid.threads()),
        (
            Err(Error::Exited { .. }),
            Err(Error::Exited { .. }),
            Err(Error::Exited { .. })
        )
    );

    Ok(())
}
