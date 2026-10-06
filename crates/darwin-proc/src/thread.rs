// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::CStr, time::Duration};

use crate::{
    Call, Error, Pid, SchedulingPolicy, Status, ffi,
    libproc::{pid_info, pid_info_into},
};

/// One thread of a process, from `proc_pidinfo` with
/// `PROC_PIDTHREADID64INFO`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadInfo {
    /// The 64-bit thread ID, which is unique across the system.
    pub id: u64,
    /// The user CPU time.
    pub user_time: Duration,
    /// The system CPU time.
    pub system_time: Duration,
    /// Recent CPU usage, in thousandths of one processor.
    pub cpu_usage: u32,
    /// The scheduling policy.
    pub policy: SchedulingPolicy,
    /// What the thread is doing.
    pub run_state: RunState,
    /// Whether the kernel has swapped the thread's stack out.
    pub swapped: bool,
    /// Whether the thread is an idle thread.
    pub idle: bool,
    /// The current scheduling priority.
    pub current_priority: i32,
    /// The base scheduling priority.
    pub base_priority: i32,
    /// The highest priority that the thread may have.
    pub max_priority: i32,
    /// The name that the thread set, or an empty string.
    pub name: String,
}

/// What a thread is doing, from `pth_run_state`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunState {
    /// `TH_STATE_RUNNING`: running or runnable.
    Running,
    /// `TH_STATE_STOPPED`: stopped.
    Stopped,
    /// `TH_STATE_WAITING`: waiting, interruptibly.
    Waiting,
    /// `TH_STATE_UNINTERRUPTIBLE`: waiting, uninterruptibly.
    Uninterruptible,
    /// `TH_STATE_HALTED`: halted at a clean point.
    Halted,
    /// A value that this crate does not know.
    Other(i32),
}

impl From<i32> for RunState {
    /// Converts a `TH_STATE_*` value from `<mach/thread_info.h>`.
    ///
    /// ```
    /// use darwin_proc::RunState;
    ///
    /// assert_eq!(
    ///     (RunState::from(3), RunState::from(9)),
    ///     (RunState::Waiting, RunState::Other(9))
    /// );
    /// ```
    fn from(state: i32) -> Self {
        match state {
            libc::TH_STATE_RUNNING => Self::Running,
            libc::TH_STATE_STOPPED => Self::Stopped,
            libc::TH_STATE_WAITING => Self::Waiting,
            libc::TH_STATE_UNINTERRUPTIBLE => Self::Uninterruptible,
            libc::TH_STATE_HALTED => Self::Halted,
            other => Self::Other(other),
        }
    }
}

impl ThreadInfo {
    /// Decodes `raw`, the data for the thread `id`. Its CPU times are in
    /// nanoseconds.
    pub(crate) fn decode(id: u64, raw: &libc::proc_threadinfo) -> Self {
        let name: Vec<u8> = raw
            .pth_name
            .iter()
            .map(|byte| byte.cast_unsigned())
            .collect();

        Self {
            id,
            user_time: Duration::from_nanos(raw.pth_user_time),
            system_time: Duration::from_nanos(raw.pth_system_time),
            cpu_usage: raw.pth_cpu_usage.cast_unsigned(),
            policy: SchedulingPolicy::from(raw.pth_policy),
            run_state: RunState::from(raw.pth_run_state),
            swapped: raw.pth_flags & libc::TH_FLAGS_SWAPPED != 0,
            idle: raw.pth_flags & libc::TH_FLAGS_IDLE != 0,
            current_priority: raw.pth_curpri,
            base_priority: raw.pth_priority,
            max_priority: raw.pth_maxpriority,
            name: CStr::from_bytes_until_nul(&name).map_or_else(
                |_| String::from_utf8_lossy(&name).into_owned(),
                |name| name.to_string_lossy().into_owned(),
            ),
        }
    }
}

impl Pid {
    /// Reads every thread of the process.
    ///
    /// A thread that exits between the call that lists the IDs and the call
    /// that reads the thread is left out.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert!(!Pid::current().threads()?.is_empty());
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for a zombie, [`Error::Denied`] for another
    /// user's process, or another error if `proc_pidinfo` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn threads(self) -> Result<Vec<ThreadInfo>, Error> {
        let ids = self.thread_ids()?;
        let mut threads = Vec::with_capacity(ids.len());
        let mut skipped = false;

        for id in ids {
            match pid_info::<libc::proc_threadinfo>(self, ffi::PROC_PIDTHREADID64INFO, id) {
                Ok(raw) => threads.push(ThreadInfo::decode(id, &raw)),
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                    tracing::debug!(thread = id, "the thread was gone when it was read");
                    skipped = true;
                }
                Err(source) => return Err(self.live_error(Call::ThreadInfo, source)),
            }
        }

        // The read of a thread also fails with `ESRCH` when the process has
        // exited or become a zombie since its threads were listed.
        if skipped {
            let info = self.info()?;

            if info.status == Status::Zombie {
                return Err(Error::Unsupported {
                    pid: self,
                    call: Call::ThreadInfo,
                });
            }
        }

        Ok(threads)
    }

    /// The IDs of the process's threads. `proc_pidinfo` writes only as many IDs
    /// as fit, so a full buffer may be missing some. The function then doubles
    /// the buffer and reads again.
    fn thread_ids(self) -> Result<Vec<u64>, Error> {
        let mut ids = vec![0_u64; 64];

        loop {
            let written = pid_info_into(self, ffi::PROC_PIDLISTTHREADIDS, 0, &mut ids)
                .map_err(|source| self.live_error(Call::ThreadList, source))?;
            let count = written / size_of::<u64>();

            if count < ids.len() {
                ids.truncate(count);
                return Ok(ids);
            }

            ids = vec![0; ids.len() * 2];
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{RunState, ThreadInfo};
    use crate::{SchedulingPolicy, sysctl::zeroed};

    #[test]
    fn every_field_is_decoded() {
        let mut raw = zeroed::<libc::proc_threadinfo>();
        raw.pth_user_time = 1_500_000_000;
        raw.pth_system_time = 2_000_000;
        raw.pth_cpu_usage = 250;
        raw.pth_policy = 1;
        raw.pth_run_state = 4;
        raw.pth_flags = libc::TH_FLAGS_SWAPPED;
        raw.pth_curpri = 31;
        raw.pth_priority = 31;
        raw.pth_maxpriority = 63;
        for (slot, byte) in raw.pth_name.iter_mut().zip(b"worker\0") {
            *slot = byte.cast_signed();
        }

        assert_eq!(
            ThreadInfo::decode(7, &raw),
            ThreadInfo {
                id: 7,
                user_time: Duration::from_millis(1_500),
                system_time: Duration::from_millis(2),
                cpu_usage: 250,
                policy: SchedulingPolicy::Timeshare,
                run_state: RunState::Uninterruptible,
                swapped: true,
                idle: false,
                current_priority: 31,
                base_priority: 31,
                max_priority: 63,
                name: "worker".to_owned(),
            }
        );
    }

    #[rstest]
    #[case::running(1, RunState::Running)]
    #[case::stopped(2, RunState::Stopped)]
    #[case::waiting(3, RunState::Waiting)]
    #[case::uninterruptible(4, RunState::Uninterruptible)]
    #[case::halted(5, RunState::Halted)]
    #[case::unknown(9, RunState::Other(9))]
    fn a_run_state_is_named(#[case] raw: i32, #[case] expected: RunState) {
        assert_eq!(RunState::from(raw), expected);
    }
}
