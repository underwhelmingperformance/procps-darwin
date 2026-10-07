// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeSet;

use darwin_proc::Pid;
use serde::{Deserialize, Serialize};

/// A group of per-process values that a source reads together.
///
/// A source always reads a process's identity: the `kinfo_proc` record, the
/// executable path and the session. Each group adds the values from one or
/// more further calls. Some of these calls are slow, and some need permission
/// that the caller may not have.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum FieldGroup {
    /// The command line, from `KERN_PROCARGS2`.
    Arguments,
    /// The environment, from `KERN_PROCARGS2`. macOS returns it to the
    /// process's owner and to root. macOS 27 makes an exception for Apple's
    /// platform binaries, and returns their environment to no other process.
    Environment,
    /// CPU time, memory sizes, faults and I/O counts, from `PROC_PIDTASKINFO`
    /// and `proc_pid_rusage`.
    Usage,
    /// Each thread's state, priority and CPU time.
    Threads,
    /// The memory regions. A walk reads each region separately. Reading one
    /// region can take time in proportion to its size, even when little of it
    /// is resident, so a walk is slow for a process with many regions or a
    /// large file mapping.
    Regions,
    /// The open file descriptors.
    FileDescriptors,
    /// The working directory.
    WorkingDirectory,
}

impl FieldGroup {
    /// Every group, in declaration order.
    ///
    /// ```
    /// use procps_core::FieldGroup;
    ///
    /// assert_eq!(FieldGroup::ALL.first(), Some(&FieldGroup::Arguments));
    /// ```
    pub const ALL: [Self; 7] = [
        Self::Arguments,
        Self::Environment,
        Self::Usage,
        Self::Threads,
        Self::Regions,
        Self::FileDescriptors,
        Self::WorkingDirectory,
    ];
}

/// The processes that a request covers.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
enum Selection {
    /// Every process.
    #[default]
    All,
    /// Only these processes.
    Processes(BTreeSet<Pid>),
}

/// What a source reads for a snapshot: the field groups, the processes and
/// whether to read the system statistics.
///
/// The default request asks for the identity of every process and no system
/// statistics. `ps -o pid,comm` needs nothing more, so for that command the
/// source reads no arguments and walks no memory regions.
///
/// ```
/// use darwin_proc::Pid;
/// use procps_core::{FieldGroup, SnapshotRequest};
///
/// let request = SnapshotRequest::default()
///     .with(FieldGroup::Arguments)
///     .for_processes([Pid::from(1)]);
///
/// assert!(request.wants(FieldGroup::Arguments) && !request.selects(Pid::from(2)));
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    groups: BTreeSet<FieldGroup>,
    selection: Selection,
    system: bool,
}

impl SnapshotRequest {
    /// Adds `group` to the groups that the source reads.
    ///
    /// ```
    /// use procps_core::{FieldGroup, SnapshotRequest};
    ///
    /// assert!(
    ///     SnapshotRequest::default()
    ///         .with(FieldGroup::Usage)
    ///         .wants(FieldGroup::Usage)
    /// );
    /// ```
    #[must_use]
    pub fn with(mut self, group: FieldGroup) -> Self {
        self.groups.insert(group);
        self
    }

    /// Limits the request to `pids`. A second call adds to the processes that
    /// the first call selected.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::SnapshotRequest;
    ///
    /// let request = SnapshotRequest::default().for_processes([Pid::from(1)]);
    ///
    /// assert!(request.selects(Pid::from(1)) && !request.selects(Pid::from(2)));
    /// ```
    #[must_use]
    pub fn for_processes(mut self, pids: impl IntoIterator<Item = Pid>) -> Self {
        match &mut self.selection {
            Selection::All => self.selection = Selection::Processes(pids.into_iter().collect()),
            Selection::Processes(selected) => selected.extend(pids),
        }

        self
    }

    /// Asks the source to read the system statistics as well.
    ///
    /// ```
    /// use procps_core::SnapshotRequest;
    ///
    /// assert!(SnapshotRequest::default().with_system().wants_system());
    /// ```
    #[must_use]
    pub const fn with_system(mut self) -> Self {
        self.system = true;
        self
    }

    /// Whether the source reads `group`.
    ///
    /// ```
    /// use procps_core::{FieldGroup, SnapshotRequest};
    ///
    /// assert!(!SnapshotRequest::default().wants(FieldGroup::Threads));
    /// ```
    #[must_use]
    pub fn wants(&self, group: FieldGroup) -> bool {
        self.groups.contains(&group)
    }

    /// Whether the request selects the process `pid`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::SnapshotRequest;
    ///
    /// assert!(SnapshotRequest::default().selects(Pid::from(1)));
    /// ```
    #[must_use]
    pub fn selects(&self, pid: Pid) -> bool {
        match &self.selection {
            Selection::All => true,
            Selection::Processes(selected) => selected.contains(&pid),
        }
    }

    /// The processes that the request selects, or `None` if it selects every
    /// process.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::SnapshotRequest;
    ///
    /// let request = SnapshotRequest::default().for_processes([Pid::from(2), Pid::from(1)]);
    ///
    /// assert_eq!(
    ///     request.selected().map(Iterator::collect::<Vec<_>>),
    ///     Some(vec![Pid::from(1), Pid::from(2)])
    /// );
    /// ```
    #[must_use]
    pub fn selected(&self) -> Option<impl Iterator<Item = Pid> + '_> {
        match &self.selection {
            Selection::All => None,
            Selection::Processes(selected) => Some(selected.iter().copied()),
        }
    }

    /// Whether the source reads the system statistics.
    ///
    /// ```
    /// use procps_core::SnapshotRequest;
    ///
    /// assert!(!SnapshotRequest::default().wants_system());
    /// ```
    #[must_use]
    pub const fn wants_system(&self) -> bool {
        self.system
    }
}
