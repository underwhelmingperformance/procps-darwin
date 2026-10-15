// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Values that procps-ng reads from Linux's `/proc` and that the tools
//! compute from a snapshot instead.

use std::{
    ffi::OsString,
    time::{Duration, SystemTime},
};

use darwin_proc::Pid;
use procps_core::{Process, Snapshot};

/// Times as procps-ng reads them on Linux: in clock ticks since boot, at 100
/// ticks a second. Two processes that start in the same tick have the same
/// start time.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Clock {
    /// The time of boot, by the wall clock.
    boot: SystemTime,
    /// The ticks between boot and the snapshot, like procps-ng's `boot_tics`.
    now: u64,
}

impl Clock {
    /// Linux's `USER_HZ`.
    const HERTZ: u64 = 100;

    /// The clock of `snapshot`.
    pub(crate) fn new(snapshot: &Snapshot) -> Self {
        Self {
            boot: snapshot
                .taken
                .checked_sub(snapshot.uptime)
                .unwrap_or(SystemTime::UNIX_EPOCH),
            now: Self::ticks(snapshot.uptime),
        }
    }

    /// The whole ticks in `duration`.
    fn ticks(duration: Duration) -> u64 {
        let nanoseconds_per_tick = u128::from(1_000_000_000 / Self::HERTZ);

        u64::try_from(duration.as_nanos() / nanoseconds_per_tick).unwrap_or(u64::MAX)
    }

    /// The tick in which `start` falls, as procps-ng's `TICS_BEGAN` item
    /// gives a process's start time, or 0 if `start` is before boot.
    pub(crate) fn tick(self, start: SystemTime) -> u64 {
        start.duration_since(self.boot).map_or(0, Self::ticks)
    }

    /// The whole seconds between `start` and the snapshot, as `pgrep`'s
    /// `(int)` cast of procps-ng's `TIME_ELAPSED` item truncates them, or 0 if
    /// `start` is later.
    pub(crate) fn elapsed_seconds(self, start: SystemTime) -> i64 {
        let elapsed = self.now.saturating_sub(self.tick(start)) / Self::HERTZ;

        i64::try_from(elapsed).unwrap_or(i64::MAX)
    }
}

/// The session of `process`. Darwin's `getsid` fails for a zombie, whereas
/// Linux reports a zombie's session. A process group is always in one
/// session, so when the source could not read the session of `process`, the
/// session of another process in its group is used.
pub(crate) fn session(snapshot: &Snapshot, process: &Process) -> Option<Pid> {
    if let Some(&session) = process.session.available() {
        return Some(session);
    }

    snapshot
        .processes
        .iter()
        .filter(|member| member.info.pgid == process.info.pgid)
        .find_map(|member| member.session.available().copied())
}

/// The name of the process's controlling terminal, as procps-ng's `TTY_NAME`
/// item gives it, such as `ttys001`, or `?` without one.
pub(crate) fn terminal_name(process: &Process) -> OsString {
    process
        .info
        .terminal
        .and_then(|terminal| terminal.name())
        .unwrap_or_else(|| OsString::from("?"))
}
