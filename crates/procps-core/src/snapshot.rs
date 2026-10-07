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
    LoadAverage, Memory, Pid, ProcessInfo, ProcessorTicks, Region, ResourceCounters, ResourceUsage,
    ShareMode, Swap, TaskInfo, TaskTotals, ThreadInfo,
};
use serde::{Deserialize, Serialize};

use crate::{Field, FieldGroup, SnapshotRequest, path_field};

/// The processes, and optionally the system statistics, that a source read in
/// one pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Process {
    /// The `kinfo_proc` record, which any user can read for every process.
    pub info: ProcessInfo,
    /// The path of the executable.
    #[serde(with = "path_field")]
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
    /// The region totals. The helper sends them to every user.
    pub region_totals: Option<Field<RegionTotals>>,
    /// The memory regions, in address order. The helper sends them only to the
    /// process's owner and to root. [`Process::with_regions`] sets them
    /// together with their totals.
    pub regions: Option<Field<Vec<Region>>>,
    /// The open file descriptors, in ascending order.
    pub file_descriptors: Option<Field<Vec<RawFd>>>,
    /// The working directory.
    #[serde(with = "path_field::optional")]
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
            region_totals: None,
            regions: None,
            file_descriptors: None,
            working_directory: None,
        }
    }
}

impl Process {
    /// This process with exactly the groups that `request` asks for. A
    /// requested group that the process lacks is unsupported. A process with
    /// regions but no region totals gets the totals of its regions.
    pub(crate) fn limited_to(self, request: &SnapshotRequest) -> Self {
        fn group<T>(value: Option<T>, wanted: bool, missing: T) -> Option<T> {
            wanted.then(|| value.unwrap_or(missing))
        }

        let wants = |group| request.wants(group);
        let region_totals = self.region_totals.or_else(|| {
            self.regions.as_ref().map(|regions| {
                regions
                    .as_ref()
                    .map(|regions| RegionTotals::from(regions.as_slice()))
            })
        });
        let unsupported = Usage {
            task: Field::Unsupported,
            resources: Field::Unsupported,
            counters: Field::Unsupported,
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
            region_totals: group(
                region_totals,
                wants(FieldGroup::Regions),
                Field::Unsupported,
            ),
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

impl Process {
    /// This process with `regions` and their totals.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Field, Process, RegionTotals};
    ///
    /// let info = Pid::current().info()?;
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported)
    ///     .with_regions(Field::Available(Vec::new()));
    ///
    /// assert_eq!(
    ///     process.region_totals,
    ///     Some(Field::Available(RegionTotals {
    ///         executable: 0,
    ///         private_writable: 0
    ///     }))
    /// );
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn with_regions(self, regions: Field<Vec<Region>>) -> Self {
        Self {
            region_totals: Some(
                regions
                    .as_ref()
                    .map(|regions| RegionTotals::from(regions.as_slice())),
            ),
            regions: Some(regions),
            ..self
        }
    }

    /// An estimate of the memory that this process's values use. It leaves out
    /// the memory regions, which the source counts as it reads them.
    pub(crate) fn estimated_size(&self) -> usize {
        /// The memory that `strings` use.
        fn strings(strings: Option<&Field<Vec<OsString>>>) -> usize {
            strings.and_then(Field::available).map_or(0, |strings| {
                strings
                    .iter()
                    .map(|string| size_of::<OsString>() + string.len())
                    .sum()
            })
        }

        /// The memory that the items of `list` use.
        fn list<T>(list: Option<&Field<Vec<T>>>) -> usize {
            list.and_then(Field::available)
                .map_or(0, |list| list.len() * size_of::<T>())
        }

        let path = |path: Option<&Field<PathBuf>>| {
            path.and_then(Field::available)
                .map_or(0, |path| path.as_os_str().len())
        };

        let thread_names = self
            .threads
            .as_ref()
            .and_then(Field::available)
            .map_or(0, |threads| {
                threads.iter().map(|thread| thread.name.len()).sum()
            });

        size_of::<Self>()
            + self.info.comm.len()
            + path(Some(&self.executable))
            + strings(self.arguments.as_ref())
            + strings(self.environment.as_ref())
            + list(self.threads.as_ref())
            + thread_names
            + list(self.file_descriptors.as_ref())
            + path(self.working_directory.as_ref())
    }
}

/// The totals of a process's memory regions. procps-ng's `trs`, `drs` and
/// `size` columns and `top`'s `CODE` and `DATA` fields use them. The regions
/// that map a submap, such as the shared cache of system libraries, are left
/// out.
///
/// ```
/// use darwin_proc::Pid;
/// use procps_core::RegionTotals;
///
/// let totals = RegionTotals::from(Pid::current().regions()?.as_slice());
///
/// assert!(totals.executable > 0);
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegionTotals {
    /// The size of the executable regions, which contain code.
    pub executable: u64,
    /// The size of the private writable regions, which contain data and
    /// stacks. procps-ng's `size` is Linux's `VmData` plus `VmStk`, which count
    /// only private writable mappings, so the regions that Darwin reports as
    /// shared are left out.
    pub private_writable: u64,
}

impl From<&[Region]> for RegionTotals {
    fn from(regions: &[Region]) -> Self {
        let outside_submaps = regions.iter().filter(|region| !region.is_submap);
        let shared = |region: &Region| {
            matches!(
                region.share_mode,
                ShareMode::Shared | ShareMode::TrueShared | ShareMode::SharedAliased
            )
        };

        Self {
            executable: outside_submaps
                .clone()
                .filter(|region| region.protection.is_executable())
                .map(|region| region.size)
                .sum(),
            private_writable: outside_submaps
                .filter(|region| region.protection.is_writable() && !shared(region))
                .map(|region| region.size)
                .sum(),
        }
    }
}

/// A process's CPU time, memory and I/O, from `PROC_PIDTASKINFO` and
/// `proc_pid_rusage`, which need the same permission. For a zombie, only
/// `proc_pid_rusage` succeeds, and procps-ng shows a zombie's CPU time, so each
/// call has its own field. The result of `proc_pid_rusage` fills two fields, so
/// that the helper can withhold the counters and still send the rest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// What `PROC_PIDTASKINFO` reports.
    pub task: Field<TaskInfo>,
    /// The CPU time, wakeups, page-ins and memory sizes that
    /// `proc_pid_rusage` reports.
    pub resources: Field<ResourceUsage>,
    /// The disk I/O, instruction, cycle and energy counters that
    /// `proc_pid_rusage` reports. The helper sends them only to the process's
    /// owner and to root.
    pub counters: Field<ResourceCounters>,
}

/// The system statistics for `top`'s summary area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
