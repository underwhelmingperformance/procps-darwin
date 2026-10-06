// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::OsString,
    time::{Duration, SystemTime},
};

use darwin_proc::{Arguments, Host, Pid, ProcessInfo};

use crate::{Field, FieldGroup, Process, Snapshot, SnapshotRequest, System, Usage};

/// Takes snapshots of processes and the system.
///
/// The tools read process data only through this trait, so tests can
/// substitute a [`FixtureSource`].
pub trait ProcessSource {
    /// Reads what `request` asks for.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] if the source cannot list the processes, read
    /// the time since boot or read the system statistics. A failure to read
    /// one value for one process is not an error: the value is a [`Field`]
    /// that records why the source has no value.
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError>;
}

/// An error that prevents a source from taking a snapshot.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The source could not list the processes.
    #[error("cannot list processes")]
    ProcessTable(#[source] darwin_proc::Error),
    /// The source could not read the time since boot.
    #[error("cannot read the time since boot")]
    Clock(#[source] darwin_proc::Error),
    /// The source could not read the system statistics.
    #[error("cannot read system statistics")]
    System(#[source] darwin_proc::Error),
}

/// Reads processes with `darwin-proc` directly, in the calling process,
/// without the helper.
///
/// Run as root, it reads everything that root can read. Otherwise it reads the
/// caller's own processes and the identity of every other process.
///
/// ```
/// use darwin_proc::Pid;
/// use procps_core::{LocalSource, ProcessSource, SnapshotRequest};
///
/// let request = SnapshotRequest::default().for_processes([Pid::current()]);
/// let snapshot = LocalSource.snapshot(&request)?;
///
/// assert_eq!(snapshot.processes.len(), 1);
/// # Ok::<(), procps_core::SourceError>(())
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalSource;

/// The process exited while the source read it.
struct Exited;

/// Converts the result of one read for `pid` into a field.
fn field<T>(pid: Pid, result: Result<T, darwin_proc::Error>) -> Result<Field<T>, Exited> {
    match result {
        Ok(value) => Ok(Field::Available(value)),
        Err(darwin_proc::Error::Exited { .. }) => Err(Exited),
        Err(darwin_proc::Error::Denied { .. }) => Ok(Field::Denied),
        Err(darwin_proc::Error::Unsupported { .. }) => Ok(Field::Unsupported),
        Err(error) => {
            tracing::warn!(%pid, %error, "read failed");
            Ok(Field::Failed)
        }
    }
}

impl LocalSource {
    /// The `kinfo_proc` records of the processes that `request` selects. A
    /// selected process that does not exist is left out.
    fn infos(request: &SnapshotRequest) -> Result<Vec<ProcessInfo>, SourceError> {
        let Some(selected) = request.selected() else {
            return ProcessInfo::all().map_err(SourceError::ProcessTable);
        };

        let mut infos = Vec::new();

        for pid in selected {
            match pid.info() {
                Ok(info) => infos.push(info),
                Err(darwin_proc::Error::Exited { .. }) => {}
                Err(error) => return Err(SourceError::ProcessTable(error)),
            }
        }

        Ok(infos)
    }

    /// Reads the groups that `request` asks for, for the process that `info`
    /// describes.
    fn process(info: ProcessInfo, request: &SnapshotRequest) -> Result<Process, Exited> {
        let pid = info.pid;
        let mut process = Process::from_identity(
            info,
            field(pid, pid.executable_path())?,
            field(pid, pid.session())?,
        );

        let wants_arguments = request.wants(FieldGroup::Arguments);
        let wants_environment = request.wants(FieldGroup::Environment);

        if wants_arguments || wants_environment {
            let arguments = field(pid, pid.arguments())?;

            if wants_arguments {
                process.arguments = Some(arguments.clone().map(|read| read.arguments));
            }

            if wants_environment {
                process.environment = Some(Self::environment(pid, arguments));
            }
        }

        if request.wants(FieldGroup::Usage) {
            process.usage = Some(Usage {
                task: field(pid, pid.task_info())?,
                resources: field(pid, pid.resource_usage())?,
            });
        }

        if request.wants(FieldGroup::Threads) {
            process.threads = Some(field(pid, pid.threads())?);
        }

        if request.wants(FieldGroup::Regions) {
            process.regions = Some(field(pid, pid.regions())?);
        }

        if request.wants(FieldGroup::FileDescriptors) {
            process.file_descriptors = Some(field(pid, pid.file_descriptors())?);
        }

        if request.wants(FieldGroup::WorkingDirectory) {
            process.working_directory = Some(field(pid, pid.working_directory())?);
        }

        Ok(process)
    }

    /// The environment from `arguments`, which the source read for `pid`.
    ///
    /// macOS 27 withholds the environment of an Apple platform binary from
    /// other processes and returns no strings in its place, or only the first
    /// string if the binary's `argv[0]` is empty. Such an environment of
    /// another process is therefore denied. So is the environment of another
    /// process that really has no strings, or that has one string while the
    /// process's `argv[0]` is empty.
    fn environment(pid: Pid, arguments: Field<Arguments>) -> Field<Vec<OsString>> {
        let Field::Available(read) = arguments else {
            return arguments.map(|read| read.environment);
        };

        let empty_first_argument = read.arguments.first().is_some_and(|first| first.is_empty());
        let withheld = match read.environment.len() {
            0 => true,
            1 => empty_first_argument,
            _ => false,
        };

        if withheld && pid != Pid::current() {
            return Field::Denied;
        }

        Field::Available(read.environment)
    }

    /// Reads the system statistics.
    fn system() -> Result<System, darwin_proc::Error> {
        Ok(System {
            boot_time: Host::boot_time()?,
            load_average: Host::load_average()?,
            memory: Host::memory()?,
            swap: Host::swap()?,
            processors: Host::processors()?,
            tasks: Host::task_totals()?,
            users: Host::logged_in_users(),
        })
    }
}

impl ProcessSource for LocalSource {
    #[tracing::instrument(level = "debug", skip(self), err(level = "debug"))]
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        let taken = SystemTime::now();
        let uptime = Host::uptime().map_err(SourceError::Clock)?;
        let mut processes: Vec<Process> = Self::infos(request)?
            .into_iter()
            .filter_map(|info| Self::process(info, request).ok())
            .collect();
        processes.sort_by_key(|process| process.info.pid);

        let system = if request.wants_system() {
            Some(Self::system().map_err(SourceError::System)?)
        } else {
            None
        };

        Ok(Snapshot {
            taken,
            uptime,
            processes,
            system,
        })
    }
}

/// Returns fixed processes and system statistics, for tests.
///
/// A snapshot contains the stored processes that the request selects, in
/// ascending order of process ID, with exactly the groups that the request
/// asks for. A requested group that a stored process lacks is unsupported. The
/// source cannot leave the group as `None`, because `None` means that the
/// request did not ask for the group, and `LocalSource` fills every group that
/// a request asks for.
///
/// ```
/// use std::time::{Duration, SystemTime};
///
/// use darwin_proc::Pid;
/// use procps_core::{Field, FixtureSource, Process, ProcessSource, SnapshotRequest};
///
/// let info = Pid::current().info()?;
/// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
/// let source = FixtureSource::new(
///     SystemTime::UNIX_EPOCH,
///     Duration::ZERO,
///     vec![process.clone()],
/// );
///
/// assert_eq!(
///     source.snapshot(&SnapshotRequest::default())?.processes,
///     vec![process]
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct FixtureSource {
    taken: SystemTime,
    uptime: Duration,
    processes: Vec<Process>,
    system: Option<System>,
}

impl FixtureSource {
    /// A source whose snapshots contain `processes`, and report the time
    /// `taken` and the time since boot `uptime`.
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use procps_core::{FixtureSource, ProcessSource, SnapshotRequest};
    ///
    /// let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
    ///
    /// assert_eq!(
    ///     source.snapshot(&SnapshotRequest::default())?.taken,
    ///     SystemTime::UNIX_EPOCH
    /// );
    /// # Ok::<(), procps_core::SourceError>(())
    /// ```
    #[must_use]
    pub const fn new(taken: SystemTime, uptime: Duration, processes: Vec<Process>) -> Self {
        Self {
            taken,
            uptime,
            processes,
            system: None,
        }
    }

    /// Adds `system` to the snapshots of requests that ask for the system
    /// statistics.
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    ///
    /// use procps_core::{FixtureSource, LocalSource, ProcessSource, SnapshotRequest};
    ///
    /// let request = SnapshotRequest::default().for_processes([]).with_system();
    /// let system = LocalSource.snapshot(&request)?.system;
    /// let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, Vec::new());
    /// let source = system
    ///     .clone()
    ///     .map_or(source.clone(), |system| source.with_system(system));
    ///
    /// assert_eq!(source.snapshot(&request)?.system, system);
    /// # Ok::<(), procps_core::SourceError>(())
    /// ```
    #[must_use]
    pub fn with_system(mut self, system: System) -> Self {
        self.system = Some(system);
        self
    }
}

impl ProcessSource for FixtureSource {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
        let mut processes: Vec<Process> = self
            .processes
            .iter()
            .filter(|process| request.selects(process.info.pid))
            .cloned()
            .map(|process| process.limited_to(request))
            .collect();
        processes.sort_by_key(|process| process.info.pid);

        Ok(Snapshot {
            taken: self.taken,
            uptime: self.uptime,
            processes,
            system: self.system.clone().filter(|_| request.wants_system()),
        })
    }
}
