// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

use darwin_proc::{RunState, SchedulingPolicy, Status};

use crate::{Field, Process};

/// A process's state in the form of procps-ng's `stat` column: a state
/// letter followed by modifiers.
///
/// ```
/// use darwin_proc::Pid;
/// use procps_core::{Field, Process};
///
/// let info = Pid::current().info()?;
/// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
///
/// assert_eq!(process.linux_state().letter(), '-');
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxState {
    letter: Option<char>,
    modifiers: String,
}

impl LinuxState {
    /// The state letter, as in procps-ng's `s` column, or `-` if the source
    /// could not read the data that decides it.
    ///
    /// ```
    /// use darwin_proc::{Pid, Status};
    /// use procps_core::{Field, Process};
    ///
    /// let mut info = Pid::current().info()?;
    /// info.status = Status::Zombie;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_state().letter(), 'Z');
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn letter(&self) -> char {
        self.letter.unwrap_or('-')
    }
}

impl fmt::Display for LinuxState {
    /// Writes the state letter and the modifiers, as in procps-ng's `stat`
    /// column.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.letter(), self.modifiers)
    }
}

/// A Linux scheduling policy that a Darwin policy corresponds to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinuxPolicy {
    /// `SCHED_OTHER`, for Darwin's `POLICY_TIMESHARE`.
    Other,
    /// `SCHED_FIFO`, for Darwin's `POLICY_FIFO`.
    Fifo,
    /// `SCHED_RR`, for Darwin's `POLICY_RR`.
    RoundRobin,
}

/// A process's priority and nice value on Linux's scales, which procps-ng's
/// priority columns are computed from.
///
/// ```
/// use procps_core::{LinuxPolicy, LinuxPriority};
///
/// let priority = LinuxPriority {
///     policy: Some(LinuxPolicy::Other),
///     priority: 20,
///     nice: 0,
///     rt_priority: Some(0),
/// };
///
/// assert_eq!(priority.class(), "TS");
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LinuxPriority {
    /// The scheduling policy, or `None` if the source could not read it.
    pub policy: Option<LinuxPolicy>,
    /// The value of the `priority` field in Linux's `/proc/<pid>/stat`: 20
    /// plus the nice value for `SCHED_OTHER`, and -1 minus the real-time
    /// priority for `SCHED_FIFO` and `SCHED_RR`.
    pub priority: i32,
    /// The nice value.
    pub nice: i8,
    /// The real-time priority: 0 for `SCHED_OTHER`, 1 to 99 for `SCHED_FIFO`
    /// and `SCHED_RR`, and `None` if the policy is unknown.
    pub rt_priority: Option<i32>,
}

impl LinuxPriority {
    /// The policy's name in procps-ng's `class` and `policy` columns.
    ///
    /// ```
    /// use procps_core::LinuxPriority;
    ///
    /// let unknown = LinuxPriority {
    ///     policy: None,
    ///     priority: 20,
    ///     nice: 0,
    ///     rt_priority: None,
    /// };
    ///
    /// assert_eq!(unknown.class(), "-");
    /// ```
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self.policy {
            Some(LinuxPolicy::Other) => "TS",
            Some(LinuxPolicy::Fifo) => "FF",
            Some(LinuxPolicy::RoundRobin) => "RR",
            None => "-",
        }
    }
}

/// `PF_FORKNOEXEC` from Linux's `<linux/sched.h>`: `fork` created the process,
/// and it has not called `execve` since.
const PF_FORKNOEXEC: u64 = 0x40;

/// `PF_SUPERPRIV` from Linux's `<linux/sched.h>`: the process used superuser
/// privileges.
const PF_SUPERPRIV: u64 = 0x100;

/// A process's sizes in bytes, for procps-ng's memory columns.
///
/// Each size is `None` when the request did not ask for a group that the size
/// comes from.
///
/// ```
/// use darwin_proc::Pid;
/// use procps_core::{Field, LinuxSizes, Process};
///
/// let info = Pid::current().info()?;
/// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
///
/// assert_eq!(process.linux_sizes(), LinuxSizes::default());
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinuxSizes {
    /// The virtual size, for `vsz`, `sz` and `m_size`, from `PROC_PIDTASKINFO`.
    pub virtual_size: Option<Field<u64>>,
    /// The resident size, for `rss`, from `PROC_PIDTASKINFO`.
    pub resident: Option<Field<u64>>,
    /// The size of the code, for `trs`: the executable regions outside
    /// submaps. This leaves out the shared cache of system libraries.
    pub text: Option<Field<u64>>,
    /// The virtual size minus the size of the code, for `drs`.
    pub data: Option<Field<u64>>,
    /// The private writable regions outside submaps, for `size`, which is the
    /// data and stack size on Linux.
    pub data_and_stack: Option<Field<u64>>,
    /// The resident memory that only this process uses, for `uss`.
    pub unique: Option<Field<u64>>,
    /// The proportional set size, for `pss`. Darwin does not report how many
    /// processes share each page, so it is always unsupported.
    pub proportional: Option<Field<u64>>,
}

/// The longest command name on Linux, in bytes: `TASK_COMM_LEN` minus the
/// terminating NUL.
const COMMAND_NAME_LENGTH: usize = 15;

/// The highest real-time priority on Linux.
const MAXIMUM_RT_PRIORITY: i32 = 99;

impl Process {
    /// The process's state as procps-ng shows it.
    ///
    /// A zombie is `Z`, and a stopped process is `T`, or `t` if it is also
    /// traced. Otherwise Darwin reports `SRUN` for almost every live process,
    /// so the threads decide the letter: `R` if any thread is running, `D` if
    /// any is in an uninterruptible wait, and `S` otherwise. Without the
    /// threads, the number of running threads in the usage group decides
    /// between `R` and `S`. Without either, the letter is `-`. Darwin does
    /// not report locked pages separately, so the state never has
    /// procps-ng's `L` modifier.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert!(process.linux_state().to_string().starts_with('-'));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_state(&self) -> LinuxState {
        let mut modifiers = String::new();

        if self.info.nice < 0 {
            modifiers.push('<');
        } else if self.info.nice > 0 {
            modifiers.push('N');
        }

        if self.info.session_leader {
            modifiers.push('s');
        }

        if self.thread_count().is_some_and(|count| count > 1) {
            modifiers.push('l');
        }

        if self
            .info
            .terminal
            .is_some_and(|terminal| terminal.foreground_group == self.info.pgid)
        {
            modifiers.push('+');
        }

        LinuxState {
            letter: self.state_letter(),
            modifiers,
        }
    }

    /// The state letter, or `None` if the source could not read the data that
    /// decides it.
    fn state_letter(&self) -> Option<char> {
        match self.info.status {
            Status::Zombie => return Some('Z'),
            Status::Stopped if self.info.flags.is_traced() => return Some('t'),
            Status::Stopped => return Some('T'),
            _ => {}
        }

        if let Some(Field::Available(threads)) = &self.threads {
            let states = || threads.iter().map(|thread| thread.run_state);

            if states().any(|state| state == RunState::Running) {
                return Some('R');
            }

            if states().any(|state| state == RunState::Uninterruptible) {
                return Some('D');
            }

            return Some('S');
        }

        let task = self.usage.as_ref()?.task.available()?;

        Some(if task.running_threads > 0 { 'R' } else { 'S' })
    }

    /// The number of threads, from the threads group or the usage group.
    fn thread_count(&self) -> Option<usize> {
        if let Some(Field::Available(threads)) = &self.threads {
            return Some(threads.len());
        }

        let task = self.usage.as_ref()?.task.available()?;

        usize::try_from(task.threads).ok()
    }

    /// The process's priority and nice value on Linux's scales.
    ///
    /// A timesharing process has `SCHED_OTHER`'s priority of 20 plus its nice
    /// value. A fixed-priority process has its Darwin base priority as its
    /// real-time priority, limited to Linux's range of 1 to 99. Without the
    /// usage group, the policy is unknown, and the priority is that of
    /// `SCHED_OTHER`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let nice = info.nice;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_priority().priority, 20 + i32::from(nice));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_priority(&self) -> LinuxPriority {
        let nice = self.info.nice;
        let other = LinuxPriority {
            policy: None,
            priority: 20 + i32::from(nice),
            nice,
            rt_priority: None,
        };

        let Some(task) = self.usage.as_ref().and_then(|usage| usage.task.available()) else {
            return other;
        };

        let policy = match task.policy {
            SchedulingPolicy::Fifo => LinuxPolicy::Fifo,
            SchedulingPolicy::RoundRobin => LinuxPolicy::RoundRobin,
            SchedulingPolicy::Timeshare => {
                return LinuxPriority {
                    policy: Some(LinuxPolicy::Other),
                    rt_priority: Some(0),
                    ..other
                };
            }
            SchedulingPolicy::Other(_) => return other,
        };

        let rt_priority = task.priority.clamp(1, MAXIMUM_RT_PRIORITY);

        LinuxPriority {
            policy: Some(policy),
            priority: -1 - rt_priority,
            nice,
            rt_priority: Some(rt_priority),
        }
    }

    /// The process's flags as Linux's `PF_*` bits, which procps-ng's `f`
    /// column and `top`'s `Flags` field show. Two bits have Darwin
    /// equivalents in the accounting flags: `PF_FORKNOEXEC` is set from
    /// `AFORK`, and `PF_SUPERPRIV` from `ASU`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_flags(), 0);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub const fn linux_flags(&self) -> u64 {
        let accounting = self.info.accounting;
        let mut flags = 0;

        if accounting.forked_without_exec() {
            flags |= PF_FORKNOEXEC;
        }

        if accounting.used_superuser() {
            flags |= PF_SUPERPRIV;
        }

        flags
    }

    /// The process's sizes for procps-ng's memory columns.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_sizes().virtual_size, None);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_sizes(&self) -> LinuxSizes {
        let task = self.usage.as_ref().map(|usage| usage.task);
        let totals = self.region_totals;
        let text = totals.map(|totals| totals.map(|totals| totals.executable));
        let data = match (task, text) {
            (Some(task), Some(text)) => Some(match (task, text) {
                (Field::Available(task), Field::Available(text)) => {
                    Field::Available(task.virtual_size.saturating_sub(text))
                }
                (Field::Available(_), outcome) => outcome,
                (outcome, _) => outcome.map(|task| task.virtual_size),
            }),
            _ => None,
        };

        LinuxSizes {
            virtual_size: task.map(|task| task.map(|task| task.virtual_size)),
            resident: task.map(|task| task.map(|task| task.resident_size)),
            text,
            data,
            data_and_stack: totals.map(|totals| totals.map(|totals| totals.private_writable)),
            unique: self.regions.as_ref().map(|regions| {
                regions.as_ref().map(|regions| {
                    regions
                        .iter()
                        .filter(|region| !region.is_submap)
                        .map(|region| region.private_resident)
                        .sum()
                })
            }),
            proportional: totals.map(|_| Field::Unsupported),
        }
    }

    /// The command name as Linux reports it: at most 15 bytes of `p_comm`, cut
    /// at a character boundary. Darwin keeps up to 16 bytes in `p_comm`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let mut info = Pid::current().info()?;
    /// info.comm = "0123456789abcdef".to_owned();
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_comm(), "0123456789abcde");
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_comm(&self) -> &str {
        let comm = &self.info.comm;
        let end = (0..=COMMAND_NAME_LENGTH.min(comm.len()))
            .rev()
            .find(|&end| comm.is_char_boundary(end))
            .unwrap_or(0);

        &comm[..end]
    }
}
