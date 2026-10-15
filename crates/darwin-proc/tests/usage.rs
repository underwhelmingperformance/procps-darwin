// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads CPU, memory and thread data of live processes.

use std::{ffi::OsString, process::Command, sync::mpsc, time::Duration};

use assert_matches::assert_matches;
use darwin_proc::{Error, Pid, RunState, SchedulingPolicy, Status};
use pretty_assertions::assert_eq;

/// A single-threaded shell that uses some CPU time, then stops itself, so
/// its CPU times stop changing.
struct BusyChild(std::process::Child);

/// An error from [`BusyChild::start`].
#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Wait(#[from] rustix::io::Errno),
    #[error("the shell changed state without stopping: {0:?}")]
    NotStopped(Option<rustix::process::WaitStatus>),
}

impl BusyChild {
    /// Starts the shell and returns once it has stopped.
    fn start() -> Result<Self, StartError> {
        let child = Command::new("/bin/sh")
            .args([
                "-c",
                "i=0; while [ $i -lt 300000 ]; do i=$((i + 1)); done; kill -STOP $$",
            ])
            .spawn()?;
        let pid = rustix::process::Pid::from_child(&child);
        let changed = rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::UNTRACED)?
            .map(|(_, status)| status);

        if !changed.is_some_and(rustix::process::WaitStatus::stopped) {
            return Err(StartError::NotStopped(changed));
        }

        Ok(Self(child))
    }

    fn pid(&self) -> Result<Pid, std::num::TryFromIntError> {
        Ok(Pid::from(i32::try_from(self.0.id())?))
    }
}

impl Drop for BusyChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
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
            thread.policy,
        ),
        (1, true, true, true, SchedulingPolicy::Timeshare)
    );

    Ok(())
}

// The kernel adds a child's CPU time to its parent's when the parent reaps
// the child. Other tests reap children too, so the parent's total is at least
// this child's time.
#[test]
fn a_reaped_child_s_cpu_time_counts_for_its_parent() -> Result<(), Box<dyn std::error::Error>> {
    let child = BusyChild::start()?;
    let task = child.pid()?.task_info()?;
    drop(child);

    let (usage, _) = Pid::current().resource_usage()?;

    assert!(
        usage.children_user_time + usage.children_system_time >= task.user_time + task.system_time,
        "{usage:?} {task:?}"
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

    let names: Vec<OsString> = Pid::current()
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
fn a_thread_of_this_process_is_running() -> Result<(), Error> {
    let states: Vec<RunState> = Pid::current()
        .threads()?
        .into_iter()
        .map(|thread| thread.run_state)
        .collect();

    assert!(
        states.contains(&RunState::Running),
        "no thread is running: {states:?}"
    );

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
