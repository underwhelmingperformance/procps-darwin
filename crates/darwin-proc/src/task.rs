// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::time::Duration;

use crate::{Call, Error, Pid, ffi, libproc::pid_info, time::Timebase};

/// A process's memory sizes, CPU time and counters, from `proc_pidinfo` with
/// `PROC_PIDTASKINFO`.
///
/// ```
/// use darwin_proc::Pid;
///
/// let task = Pid::current().task_info()?;
///
/// assert!(task.threads >= 1);
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TaskInfo {
    /// The virtual memory size in bytes.
    pub virtual_size: u64,
    /// The resident memory size in bytes.
    pub resident_size: u64,
    /// The user CPU time of the process, including threads that have exited.
    pub user_time: Duration,
    /// The system CPU time of the process, including threads that have
    /// exited.
    pub system_time: Duration,
    /// The default scheduling policy.
    pub policy: SchedulingPolicy,
    /// The number of page faults.
    pub faults: u32,
    /// The number of page faults that read from disk.
    pub pageins: u32,
    /// The number of copy-on-write faults.
    pub copy_on_write_faults: u32,
    /// The number of Mach messages sent.
    pub messages_sent: u32,
    /// The number of Mach messages received.
    pub messages_received: u32,
    /// The number of Mach system calls.
    pub mach_system_calls: u32,
    /// The number of BSD system calls.
    pub unix_system_calls: u32,
    /// The number of context switches.
    pub context_switches: u32,
    /// The number of threads.
    pub threads: u32,
    /// The number of running threads.
    pub running_threads: u32,
    /// The base priority, on Darwin's scale.
    pub priority: i32,
}

/// A Mach scheduling policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SchedulingPolicy {
    /// `POLICY_TIMESHARE`, the default.
    Timeshare,
    /// `POLICY_RR`: fixed priority, round robin.
    RoundRobin,
    /// `POLICY_FIFO`: fixed priority, first in first out.
    Fifo,
    /// A value that this crate does not know.
    Other(i32),
}

impl From<i32> for SchedulingPolicy {
    /// Converts a `POLICY_*` value from `<mach/policy.h>`.
    ///
    /// ```
    /// use darwin_proc::SchedulingPolicy;
    ///
    /// assert_eq!(
    ///     (SchedulingPolicy::from(1), SchedulingPolicy::from(3)),
    ///     (SchedulingPolicy::Timeshare, SchedulingPolicy::Other(3))
    /// );
    /// ```
    fn from(policy: i32) -> Self {
        match policy {
            ffi::POLICY_TIMESHARE => Self::Timeshare,
            ffi::POLICY_RR => Self::RoundRobin,
            ffi::POLICY_FIFO => Self::Fifo,
            other => Self::Other(other),
        }
    }
}

impl TaskInfo {
    /// Decodes `raw`, whose CPU times are in Mach absolute time.
    ///
    /// The kernel keeps the counters as 32-bit `int`s, which turn negative
    /// above 2^31 - 1. Reading them as unsigned keeps them correct up to
    /// 2^32 - 1.
    pub(crate) fn decode(raw: &libc::proc_taskinfo, timebase: Timebase) -> Self {
        Self {
            virtual_size: raw.pti_virtual_size,
            resident_size: raw.pti_resident_size,
            user_time: timebase.duration(raw.pti_total_user),
            system_time: timebase.duration(raw.pti_total_system),
            policy: SchedulingPolicy::from(raw.pti_policy),
            faults: raw.pti_faults.cast_unsigned(),
            pageins: raw.pti_pageins.cast_unsigned(),
            copy_on_write_faults: raw.pti_cow_faults.cast_unsigned(),
            messages_sent: raw.pti_messages_sent.cast_unsigned(),
            messages_received: raw.pti_messages_received.cast_unsigned(),
            mach_system_calls: raw.pti_syscalls_mach.cast_unsigned(),
            unix_system_calls: raw.pti_syscalls_unix.cast_unsigned(),
            context_switches: raw.pti_csw.cast_unsigned(),
            threads: raw.pti_threadnum.cast_unsigned(),
            running_threads: raw.pti_numrunning.cast_unsigned(),
            priority: raw.pti_priority,
        }
    }
}

impl Pid {
    /// Reads the process's [`TaskInfo`].
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// assert!(Pid::current().task_info()?.resident_size > 0);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for a zombie, [`Error::Denied`] for another
    /// user's process, or another error if `proc_pidinfo` fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn task_info(self) -> Result<TaskInfo, Error> {
        let raw: libc::proc_taskinfo = pid_info(self, libc::PROC_PIDTASKINFO, 0)
            .map_err(|source| self.live_error(Call::TaskInfo, source))?;
        let timebase = Timebase::current().map_err(|source| Error::Os {
            call: Call::TaskInfo,
            source,
        })?;

        Ok(TaskInfo::decode(&raw, timebase))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{SchedulingPolicy, TaskInfo};
    use crate::{sysctl::zeroed, time::Timebase};

    #[test]
    fn every_field_is_decoded() {
        let mut raw = zeroed::<libc::proc_taskinfo>();
        raw.pti_virtual_size = 1 << 40;
        raw.pti_resident_size = 1 << 20;
        raw.pti_total_user = 24_000_000;
        raw.pti_total_system = 2_400_000;
        raw.pti_policy = 2;
        raw.pti_faults = -1;
        raw.pti_pageins = 3;
        raw.pti_cow_faults = 4;
        raw.pti_messages_sent = 5;
        raw.pti_messages_received = 6;
        raw.pti_syscalls_mach = 7;
        raw.pti_syscalls_unix = 8;
        raw.pti_csw = 9;
        raw.pti_threadnum = 10;
        raw.pti_numrunning = 2;
        raw.pti_priority = 31;

        assert_eq!(
            TaskInfo::decode(&raw, Timebase::new(125, 3)),
            TaskInfo {
                virtual_size: 1 << 40,
                resident_size: 1 << 20,
                user_time: Duration::from_secs(1),
                system_time: Duration::from_millis(100),
                policy: SchedulingPolicy::RoundRobin,
                faults: u32::MAX,
                pageins: 3,
                copy_on_write_faults: 4,
                messages_sent: 5,
                messages_received: 6,
                mach_system_calls: 7,
                unix_system_calls: 8,
                context_switches: 9,
                threads: 10,
                running_threads: 2,
                priority: 31,
            }
        );
    }

    #[rstest]
    #[case::timeshare(1, SchedulingPolicy::Timeshare)]
    #[case::round_robin(2, SchedulingPolicy::RoundRobin)]
    #[case::fifo(4, SchedulingPolicy::Fifo)]
    #[case::unknown(3, SchedulingPolicy::Other(3))]
    fn a_policy_is_named(#[case] raw: i32, #[case] expected: SchedulingPolicy) {
        assert_eq!(SchedulingPolicy::from(raw), expected);
    }
}
