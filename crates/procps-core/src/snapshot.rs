// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::OsString,
    os::fd::RawFd,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use darwin_proc::{
    LoadAverage, Memory, Pid, ProcessInfo, ProcessorTicks, Region, ResourceUsage, Swap, TaskInfo,
    TaskTotals, ThreadInfo,
};

use crate::{Field, FieldGroup, SnapshotRequest};

/// The processes, and optionally the system statistics, that a source read in
/// one pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// When the source started reading.
    pub taken: SystemTime,
    /// The time since boot, including time asleep, when the source started
    /// reading. Unlike `taken`, it never decreases, so `top` can measure
    /// the interval between two snapshots with it.
    pub uptime: Duration,
    /// The processes that the request selects, in ascending order of process
    /// ID. A process that exits before the source reads it is left out.
    pub processes: Vec<Process>,
    /// The system statistics, if the request asked for them.
    pub system: Option<System>,
}

/// What a source read about one process.
///
/// The identity is always present. Every other value is `None` when the
/// request did not ask for its [`FieldGroup`](crate::FieldGroup).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Process {
    /// The `kinfo_proc` record, which any user can read for every process.
    pub info: ProcessInfo,
    /// The path of the executable.
    pub executable: Field<PathBuf>,
    /// The session ID.
    pub session: Field<Pid>,
    /// The arguments, starting with `argv[0]`.
    pub arguments: Option<Field<Vec<OsString>>>,
    /// The environment, as `NAME=value` strings.
    pub environment: Option<Field<Vec<OsString>>>,
    /// CPU time, memory and I/O.
    pub usage: Option<Usage>,
    /// The threads.
    pub threads: Option<Field<Vec<ThreadInfo>>>,
    /// The memory regions, in address order.
    pub regions: Option<Field<Vec<Region>>>,
    /// The open file descriptors, in ascending order.
    pub file_descriptors: Option<Field<Vec<RawFd>>>,
    /// The working directory.
    pub working_directory: Option<Field<PathBuf>>,
}

impl Process {
    /// A process with only its identity, which is what a source reads for the
    /// default request.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.arguments, None);
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub const fn from_identity(
        info: ProcessInfo,
        executable: Field<PathBuf>,
        session: Field<Pid>,
    ) -> Self {
        Self {
            info,
            executable,
            session,
            arguments: None,
            environment: None,
            usage: None,
            threads: None,
            regions: None,
            file_descriptors: None,
            working_directory: None,
        }
    }
}

impl Process {
    /// This process with exactly the groups that `request` asks for. A
    /// requested group that the process lacks is unsupported.
    pub(crate) fn limited_to(self, request: &SnapshotRequest) -> Self {
        fn group<T>(value: Option<T>, wanted: bool, missing: T) -> Option<T> {
            wanted.then(|| value.unwrap_or(missing))
        }

        let wants = |group| request.wants(group);
        let unsupported = Usage {
            task: Field::Unsupported,
            resources: Field::Unsupported,
        };

        Self {
            arguments: group(
                self.arguments,
                wants(FieldGroup::Arguments),
                Field::Unsupported,
            ),
            environment: group(
                self.environment,
                wants(FieldGroup::Environment),
                Field::Unsupported,
            ),
            usage: group(self.usage, wants(FieldGroup::Usage), unsupported),
            threads: group(self.threads, wants(FieldGroup::Threads), Field::Unsupported),
            regions: group(self.regions, wants(FieldGroup::Regions), Field::Unsupported),
            file_descriptors: group(
                self.file_descriptors,
                wants(FieldGroup::FileDescriptors),
                Field::Unsupported,
            ),
            working_directory: group(
                self.working_directory,
                wants(FieldGroup::WorkingDirectory),
                Field::Unsupported,
            ),
            ..self
        }
    }
}

/// A process's CPU time, memory and I/O, from two calls that need the same
/// permission. For a zombie, only `proc_pid_rusage` succeeds, and procps-ng
/// shows a zombie's CPU time, so each call has its own field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Usage {
    /// What `PROC_PIDTASKINFO` reports.
    pub task: Field<TaskInfo>,
    /// What `proc_pid_rusage` reports.
    pub resources: Field<ResourceUsage>,
}

/// The system statistics for `top`'s summary area.
#[derive(Clone, Debug, PartialEq)]
pub struct System {
    /// When the system booted.
    pub boot_time: SystemTime,
    /// The load averages.
    pub load_average: LoadAverage,
    /// The physical memory.
    pub memory: Memory,
    /// The swap space.
    pub swap: Swap,
    /// Each processor's ticks.
    pub processors: Vec<ProcessorTicks>,
    /// The numbers of tasks and threads.
    pub tasks: TaskTotals,
    /// The number of login sessions.
    pub users: usize,
}
