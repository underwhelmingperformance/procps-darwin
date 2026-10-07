// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Processes and threads with fixed values, for tests that build snapshots
//! without reading the live process table.

use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    time::{Duration, SystemTime},
};

use darwin_proc::{
    AccountingFlags, Credentials, Gid, Pid, ProcessFlags, ProcessInfo, RunState, SchedulingPolicy,
    SignalSet, Status, ThreadInfo, Uid,
};

use crate::{Field, Process};

/// A running process with `pid` and the command name `comm`. Its parent is
/// launchd, it leads its own process group, it belongs to uid 501 and gid 20,
/// and it started at the Unix epoch. Its executable and session are
/// `Unsupported`, and its other field groups are `None`, as for a group that
/// the snapshot request left out.
///
/// ```
/// use std::ffi::OsStr;
///
/// use darwin_proc::Pid;
/// use procps_core::testing::process;
///
/// let process = process(10, "sleep");
///
/// assert_eq!(
///     (process.info.pid, process.info.comm.as_os_str()),
///     (Pid::from(10), OsStr::new("sleep"))
/// );
/// ```
#[must_use]
pub fn process(pid: i32, comm: &str) -> Process {
    let uid = Uid::from(501);
    let gid = Gid::from(20);
    let info = ProcessInfo {
        pid: Pid::from(pid),
        ppid: Pid::from(1),
        pgid: Pid::from(pid),
        terminal: None,
        credentials: Credentials {
            ruid: uid,
            euid: uid,
            svuid: uid,
            rgid: gid,
            egid: gid,
            svgid: gid,
            groups: vec![gid],
        },
        nice: 0,
        priority: 31,
        status: Status::Running,
        flags: ProcessFlags::default(),
        accounting: AccountingFlags::default(),
        session_leader: false,
        start_time: SystemTime::UNIX_EPOCH,
        comm: comm.into(),
        ignored_signals: SignalSet::default(),
        caught_signals: SignalSet::default(),
    };

    Process::from_identity(info, Field::Unsupported, Field::Unsupported)
}

/// A thread with `id`, `name` and `run_state`. It has used no CPU time and
/// has the default priorities of a timesharing thread.
///
/// ```
/// use darwin_proc::RunState;
/// use procps_core::testing::thread;
///
/// let thread = thread(7, b"io", RunState::Waiting);
///
/// assert_eq!((thread.id, thread.name.as_os_str()), (7, "io".as_ref()));
/// ```
#[must_use]
pub fn thread(id: u64, name: &[u8], run_state: RunState) -> ThreadInfo {
    ThreadInfo {
        id,
        user_time: Duration::ZERO,
        system_time: Duration::ZERO,
        cpu_usage: 0,
        policy: SchedulingPolicy::Timeshare,
        run_state,
        swapped: false,
        idle: false,
        current_priority: 31,
        base_priority: 31,
        max_priority: 63,
        name: OsString::from_vec(name.to_vec()),
    }
}
