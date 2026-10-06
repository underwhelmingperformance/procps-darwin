// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;

use darwin_proc::{Gid, Pid, ProcessInfo, Uid};

use crate::{Field, Process, ProcessSource, Snapshot, SnapshotRequest, SourceError, Usage};

/// The effective user and group IDs of the process that sent a request to the
/// helper, as `getpeereid` reports them.
///
/// The helper reads every process as root, and then withholds what Linux shows
/// only to a process's owner and to root:
///
/// - the environment, as in `/proc/<pid>/environ`;
/// - the working directory, as in `/proc/<pid>/cwd`;
/// - the file descriptors, as in `/proc/<pid>/fd`;
/// - the disk I/O counters, as in `/proc/<pid>/io`;
/// - the instruction, cycle and energy counters, which Linux does not show per
///   process;
/// - the memory regions, as in `/proc/<pid>/maps` and
///   `/proc/<pid>/smaps_rollup`.
///
/// Linux checks access to those files with `ptrace`'s read-access check, apart
/// from `fd`, which only its owner may list. This type models the check. Every
/// other value goes to every user, as `/proc/<pid>/stat`, `status`, `statm` and
/// `cmdline` do. The executable path also goes to every user, because macOS
/// gives it to every user, although Linux guards `/proc/<pid>/exe` with the
/// same check.
///
/// ```
/// use darwin_proc::{Gid, Pid, Uid};
/// use procps_core::helper::Caller;
///
/// let launchd = Pid::from(1).info()?;
/// let root = Caller::new(Uid::from(0), Gid::from(0));
///
/// assert!(root.may_read_private(&launchd));
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Caller {
    uid: Uid,
    gid: Gid,
}

impl Caller {
    /// The caller with the effective user ID `uid` and the effective group ID
    /// `gid`.
    ///
    /// ```
    /// use darwin_proc::{Gid, Pid, Uid};
    /// use procps_core::helper::Caller;
    ///
    /// let launchd = Pid::from(1).info()?;
    /// let user = Caller::new(Uid::from(501), Gid::from(20));
    ///
    /// assert!(!user.may_read_private(&launchd));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub const fn new(uid: Uid, gid: Gid) -> Self {
        Self { uid, gid }
    }

    /// Whether the caller is root.
    const fn is_root(self) -> bool {
        self.uid.as_raw() == 0
    }

    /// Whether the caller may read the private values of the process that
    /// `info` describes.
    ///
    /// Root may read every process. Another caller may read a process whose
    /// real, effective and saved user IDs all equal the caller's user ID,
    /// and whose real, effective and saved group IDs all equal the caller's
    /// group ID, as Linux requires. The process must also not have
    /// `P_SUGID` set. XNU sets it when the process changes its credentials
    /// or executes a set-user-ID or set-group-ID program, and
    /// clears it when the process executes another program. Linux denies
    /// access to a process that is not dumpable, and a process that has done
    /// either of those things is not dumpable on Linux.
    ///
    /// ```
    /// use darwin_proc::{Gid, Pid, Uid};
    /// use procps_core::helper::Caller;
    ///
    /// let info = Pid::current().info()?;
    /// let owner = Caller::new(info.credentials.euid, info.credentials.egid);
    /// let stranger = Caller::new(Uid::from(u32::MAX), info.credentials.egid);
    ///
    /// assert!(owner.may_read_private(&info) && !stranger.may_read_private(&info));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn may_read_private(self, info: &ProcessInfo) -> bool {
        let credentials = &info.credentials;
        let users = [credentials.ruid, credentials.euid, credentials.svuid];
        let groups = [credentials.rgid, credentials.egid, credentials.svgid];

        self.is_root()
            || (users == [self.uid; 3]
                && groups == [self.gid; 3]
                && !info.flags.changed_credentials())
    }

    /// `snapshot` with every private value that the caller may not read
    /// replaced by [`Field::Denied`]. A group that the request did not ask for
    /// stays out.
    ///
    /// The source read each process's credentials before its private values,
    /// and the process could have executed a set-user-ID program, or exited
    /// and had its process ID reused, in between. Unless the caller is root,
    /// this method therefore reads the identities again from `source`, and
    /// passes on a process's private values only if the caller may read the
    /// process at both readings and its start time is the same.
    ///
    /// ```
    /// use darwin_proc::{Gid, Pid, Uid};
    /// use procps_core::{
    ///     Field, FieldGroup, LocalSource, ProcessSource, SnapshotRequest, helper::Caller,
    /// };
    ///
    /// let request = SnapshotRequest::default()
    ///     .with(FieldGroup::Environment)
    ///     .for_processes([Pid::current()]);
    /// let stranger = Caller::new(Uid::from(u32::MAX), Gid::from(u32::MAX));
    /// let snapshot = stranger.redact(LocalSource.snapshot(&request)?, &LocalSource)?;
    ///
    /// assert_eq!(snapshot.processes[0].environment, Some(Field::Denied));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] if `source` cannot read the identities again.
    pub fn redact(
        self,
        snapshot: Snapshot,
        source: &impl ProcessSource,
    ) -> Result<Snapshot, SourceError> {
        if self.is_root() {
            return Ok(snapshot);
        }

        let readable = snapshot
            .processes
            .iter()
            .map(|process| &process.info)
            .filter(|info| self.may_read_private(info))
            .map(|info| info.pid);
        let later: HashMap<Pid, ProcessInfo> = source
            .snapshot(&SnapshotRequest::default().for_processes(readable))?
            .processes
            .into_iter()
            .map(|process| (process.info.pid, process.info))
            .collect();

        Ok(Snapshot {
            processes: snapshot
                .processes
                .into_iter()
                .map(|process| {
                    let unchanged = later.get(&process.info.pid).is_some_and(|after| {
                        after.start_time == process.info.start_time
                            && self.may_read_private(&process.info)
                            && self.may_read_private(after)
                    });

                    if unchanged {
                        process
                    } else {
                        withhold(process)
                    }
                })
                .collect(),
            ..snapshot
        })
    }
}

/// `process` with its private values replaced by [`Field::Denied`].
fn withhold(process: Process) -> Process {
    Process {
        environment: denied(process.environment),
        usage: process.usage.map(|usage| Usage {
            counters: Field::Denied,
            ..usage
        }),
        regions: denied(process.regions),
        file_descriptors: denied(process.file_descriptors),
        working_directory: denied(process.working_directory),
        ..process
    }
}

/// [`Field::Denied`] for a group that the request asked for.
fn denied<T>(group: Option<Field<T>>) -> Option<Field<T>> {
    group.map(|_| Field::Denied)
}
