// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::CStr,
    io,
    time::{Duration, SystemTime},
};

use crate::{
    Call, Error, Pid, SignalSet,
    ffi::{self, KinfoProc},
};

/// What `struct kinfo_proc` reports about a process. Any user can read it for
/// every process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    /// The process ID.
    pub pid: Pid,
    /// The parent's process ID.
    pub ppid: Pid,
    /// The process group ID.
    pub pgid: Pid,
    /// The controlling terminal, if there is one.
    pub terminal: Option<Terminal>,
    /// The user and group IDs.
    pub credentials: Credentials,
    /// The nice value.
    pub nice: i8,
    /// The scheduling priority, on Darwin's scale.
    pub priority: u8,
    /// The kernel's process status.
    pub status: Status,
    /// The kernel's `P_*` flags.
    pub flags: ProcessFlags,
    /// Whether the process leads its session.
    pub session_leader: bool,
    /// When the process started.
    pub start_time: SystemTime,
    /// The first 16 bytes of the executable's file name.
    pub comm: String,
    /// The signals that the process ignores.
    pub ignored_signals: SignalSet,
    /// The signals that the process catches.
    pub caught_signals: SignalSet,
}

/// A process's controlling terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Terminal {
    /// The terminal's device number.
    pub device: libc::dev_t,
    /// The process group ID of the terminal's foreground job.
    pub foreground_group: Pid,
}

/// The user and group IDs of a process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Credentials {
    /// The real user ID.
    pub ruid: u32,
    /// The effective user ID.
    pub euid: u32,
    /// The saved user ID.
    pub svuid: u32,
    /// The real group ID.
    pub rgid: u32,
    /// The effective group ID, which Darwin keeps as the first entry of the
    /// group list.
    pub egid: u32,
    /// The saved group ID.
    pub svgid: u32,
    /// The group list, starting with the effective group ID. `kinfo_proc` has
    /// room for 16 groups, so a longer list is truncated.
    pub groups: Vec<u32>,
}

/// The kernel's status for a process, from `p_stat`.
///
/// Darwin reports [`Status::Running`] for almost every live process, whether
/// it is runnable or asleep. Only the states of its threads show which.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    /// `SIDL`: being created.
    Idle,
    /// `SRUN`: runnable.
    Running,
    /// `SSLEEP`: sleeping.
    Sleeping,
    /// `SSTOP`: stopped by a signal or a debugger.
    Stopped,
    /// `SZOMB`: exited and waiting for its parent.
    Zombie,
    /// A value that this crate does not know.
    Other(i8),
}

impl From<libc::c_char> for Status {
    fn from(status: libc::c_char) -> Self {
        match status {
            ffi::SIDL => Self::Idle,
            ffi::SRUN => Self::Running,
            ffi::SSLEEP => Self::Sleeping,
            ffi::SSTOP => Self::Stopped,
            ffi::SZOMB => Self::Zombie,
            other => Self::Other(other),
        }
    }
}

/// The kernel's `P_*` flags for a process.
///
/// ```
/// use darwin_proc::ProcessFlags;
///
/// let flags = ProcessFlags::from(0x800 | 0x4);
///
/// assert_eq!(
///     (flags.is_traced(), flags.is_64_bit(), flags.is_system()),
///     (true, true, false)
/// );
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, derive_more::From)]
pub struct ProcessFlags(u32);

impl ProcessFlags {
    /// The flags as the kernel stores them.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert_eq!(ProcessFlags::from(0x4004).bits(), 0x4004);
    /// ```
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// `P_LP64`: the process is 64-bit.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x4).is_64_bit());
    /// ```
    #[must_use]
    pub const fn is_64_bit(self) -> bool {
        self.has(ffi::P_LP64)
    }

    /// `P_SUGID`: the process has changed its credentials since it last
    /// executed a program.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x100).changed_credentials());
    /// ```
    #[must_use]
    pub const fn changed_credentials(self) -> bool {
        self.has(ffi::P_SUGID)
    }

    /// `P_SYSTEM`: a system process.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x200).is_system());
    /// ```
    #[must_use]
    pub const fn is_system(self) -> bool {
        self.has(ffi::P_SYSTEM)
    }

    /// `P_TRACED`: a debugger is attached.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x800).is_traced());
    /// ```
    #[must_use]
    pub const fn is_traced(self) -> bool {
        self.has(ffi::P_TRACED)
    }

    /// `P_WEXIT`: the process is exiting.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x2000).is_exiting());
    /// ```
    #[must_use]
    pub const fn is_exiting(self) -> bool {
        self.has(ffi::P_WEXIT)
    }

    /// `P_EXEC`: the process has executed a program since it was created.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x4000).has_execed());
    /// ```
    #[must_use]
    pub const fn has_execed(self) -> bool {
        self.has(ffi::P_EXEC)
    }

    /// `P_TRANSLATED`: the process runs under Rosetta.
    ///
    /// ```
    /// use darwin_proc::ProcessFlags;
    ///
    /// assert!(ProcessFlags::from(0x20000).is_translated());
    /// ```
    #[must_use]
    pub const fn is_translated(self) -> bool {
        self.has(ffi::P_TRANSLATED)
    }

    const fn has(self, flag: libc::c_int) -> bool {
        self.0 & flag.cast_unsigned() != 0
    }
}

impl From<&KinfoProc> for ProcessInfo {
    fn from(kinfo: &KinfoProc) -> Self {
        let proc = &kinfo.kp_proc;
        let eproc = &kinfo.kp_eproc;
        let ucred = &eproc.e_ucred;
        let group_count = usize::try_from(ucred.cr_ngroups)
            .unwrap_or(0)
            .min(ffi::NGROUPS);
        let groups = ucred.cr_groups[..group_count].to_vec();
        let start = proc.p_starttime;

        Self {
            pid: Pid::from(proc.p_pid),
            ppid: Pid::from(eproc.e_ppid),
            pgid: Pid::from(eproc.e_pgid),
            terminal: (eproc.e_tdev != ffi::NODEV).then(|| Terminal {
                device: eproc.e_tdev,
                foreground_group: Pid::from(eproc.e_tpgid),
            }),
            credentials: Credentials {
                ruid: eproc.e_pcred.p_ruid,
                euid: ucred.cr_uid,
                svuid: eproc.e_pcred.p_svuid,
                rgid: eproc.e_pcred.p_rgid,
                egid: groups.first().copied().unwrap_or(eproc.e_pcred.p_rgid),
                svgid: eproc.e_pcred.p_svgid,
                groups,
            },
            nice: proc.p_nice,
            priority: proc.p_priority,
            status: Status::from(proc.p_stat),
            flags: ProcessFlags::from(proc.p_flag.cast_unsigned()),
            session_leader: eproc.e_flag & ffi::EPROC_SLEADER != 0,
            start_time: SystemTime::UNIX_EPOCH
                + Duration::from_secs(u64::try_from(start.tv_sec).unwrap_or(0))
                + Duration::from_micros(u64::try_from(start.tv_usec).unwrap_or(0)),
            comm: comm(&proc.p_comm),
            ignored_signals: SignalSet::from(proc.p_sigignore),
            caught_signals: SignalSet::from(proc.p_sigcatch),
        }
    }
}

/// The text of `p_comm`, which the kernel terminates with a NUL unless it is
/// full.
fn comm(bytes: &[libc::c_char; ffi::MAXCOMLEN + 1]) -> String {
    let bytes: Vec<u8> = bytes.iter().map(|byte| byte.cast_unsigned()).collect();

    CStr::from_bytes_until_nul(&bytes).map_or_else(
        |_| String::from_utf8_lossy(&bytes).into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl ProcessInfo {
    /// Reads every process with one `kern.proc.all` sysctl.
    ///
    /// ```
    /// use darwin_proc::{Pid, ProcessInfo};
    ///
    /// let all = ProcessInfo::all()?;
    ///
    /// assert!(all.iter().any(|info| info.pid == Pid::current()));
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the sysctl fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn all() -> Result<Vec<Self>, Error> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL, 0];

        // The table can grow between the call that measures it and the call
        // that reads it, so read with room to spare and retry if the table is
        // still too small.
        loop {
            let mut size = 0;
            sysctl(&mut mib, None, &mut size).map_err(table_error)?;

            let mut table: Vec<KinfoProc> = std::iter::repeat_with(KinfoProc::zeroed)
                .take(size / size_of::<KinfoProc>() + 32)
                .collect();
            let mut size = std::mem::size_of_val(table.as_slice());

            match sysctl(&mut mib, Some(&mut table), &mut size) {
                Ok(()) => {
                    table.truncate(size / size_of::<KinfoProc>());

                    return Ok(table.iter().map(Self::from).collect());
                }
                Err(error) if error.raw_os_error() == Some(libc::ENOMEM) => {}
                Err(error) => return Err(table_error(error)),
            }
        }
    }
}

impl Pid {
    /// Reads this process's [`ProcessInfo`].
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// let launchd = Pid::from(1).info()?;
    ///
    /// assert_eq!(
    ///     (launchd.comm.as_str(), launchd.credentials.euid),
    ///     ("launchd", 0)
    /// );
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist, or another
    /// error if the sysctl fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn info(self) -> Result<ProcessInfo, Error> {
        let mut mib = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            self.as_raw(),
        ];
        let mut kinfo = [KinfoProc::zeroed()];
        let mut size = size_of::<KinfoProc>();

        sysctl(&mut mib, Some(&mut kinfo), &mut size)
            .map_err(|source| Error::for_process(Call::ProcessTable, self, source))?;

        // The sysctl succeeds with no data for a process that does not exist.
        if size < size_of::<KinfoProc>() {
            return Err(Error::Exited { pid: self });
        }

        let [kinfo] = &kinfo;

        Ok(ProcessInfo::from(kinfo))
    }
}

fn table_error(source: io::Error) -> Error {
    Error::Os {
        call: Call::ProcessTable,
        source,
    }
}

/// Calls `sysctl` for `mib`. With `buffer`, the kernel fills it and sets
/// `size` to the number of bytes that it wrote. Without a buffer, the kernel
/// sets `size` to the number of bytes that it would write.
fn sysctl(
    mib: &mut [libc::c_int],
    buffer: Option<&mut [KinfoProc]>,
    size: &mut usize,
) -> io::Result<()> {
    let length = libc::c_uint::try_from(mib.len()).map_err(io::Error::other)?;
    let pointer = buffer.map_or(std::ptr::null_mut(), |buffer| {
        *size = (*size).min(std::mem::size_of_val(buffer));
        buffer.as_mut_ptr().cast::<libc::c_void>()
    });

    // SAFETY: `mib` is valid for `length` integers. `pointer` is either null,
    // which asks only for the size, or points to a buffer of at least `*size`
    // bytes, because `*size` is clamped to the buffer's length above.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            length,
            pointer,
            size,
            std::ptr::null_mut(),
            0,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Credentials, ProcessFlags, ProcessInfo, Status, Terminal};
    use crate::{
        Pid, SignalSet,
        ffi::{self, KinfoProc},
    };

    /// A `kinfo_proc` with a value in every field that `ProcessInfo` reads.
    fn kinfo() -> KinfoProc {
        let mut kinfo = KinfoProc::zeroed();
        let proc = &mut kinfo.kp_proc;
        proc.p_pid = 4321;
        proc.p_stat = ffi::SSTOP;
        proc.p_flag = ffi::P_LP64 | ffi::P_TRACED;
        proc.p_nice = -5;
        proc.p_priority = 31;
        proc.p_starttime = libc::timeval {
            tv_sec: 1_700_000_000,
            tv_usec: 250_000,
        };
        proc.p_sigignore = 0x10;
        proc.p_sigcatch = 0x6000;
        for (slot, byte) in proc.p_comm.iter_mut().zip(b"worker\0") {
            *slot = byte.cast_signed();
        }

        let eproc = &mut kinfo.kp_eproc;
        eproc.e_ppid = 1;
        eproc.e_pgid = 4300;
        eproc.e_tdev = 0x1000_0003;
        eproc.e_tpgid = 4310;
        eproc.e_flag = ffi::EPROC_SLEADER;
        eproc.e_pcred.p_ruid = 501;
        eproc.e_pcred.p_svuid = 502;
        eproc.e_pcred.p_rgid = 20;
        eproc.e_pcred.p_svgid = 21;
        eproc.e_ucred.cr_uid = 503;
        eproc.e_ucred.cr_ngroups = 3;
        eproc.e_ucred.cr_groups[..3].copy_from_slice(&[12, 20, 80]);

        kinfo
    }

    /// The `ProcessInfo` that `kinfo()` decodes to.
    fn expected() -> ProcessInfo {
        ProcessInfo {
            pid: Pid::from(4321),
            ppid: Pid::from(1),
            pgid: Pid::from(4300),
            terminal: Some(Terminal {
                device: 0x1000_0003,
                foreground_group: Pid::from(4310),
            }),
            credentials: Credentials {
                ruid: 501,
                euid: 503,
                svuid: 502,
                rgid: 20,
                egid: 12,
                svgid: 21,
                groups: vec![12, 20, 80],
            },
            nice: -5,
            priority: 31,
            status: Status::Stopped,
            flags: ProcessFlags::from(0x804),
            session_leader: true,
            start_time: SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_250),
            comm: "worker".to_owned(),
            ignored_signals: SignalSet::from(0x10),
            caught_signals: SignalSet::from(0x6000),
        }
    }

    #[test]
    fn every_field_is_decoded() {
        assert_eq!(ProcessInfo::from(&kinfo()), expected());
    }

    #[test]
    fn a_process_without_a_terminal_has_none() {
        let mut kinfo = kinfo();
        kinfo.kp_eproc.e_tdev = ffi::NODEV;
        kinfo.kp_eproc.e_tpgid = 0;

        assert_eq!(
            ProcessInfo::from(&kinfo),
            ProcessInfo {
                terminal: None,
                ..expected()
            }
        );
    }

    #[rstest]
    #[case::more_than_fit(20, (0..16).collect(), 0)]
    #[case::negative(-1, Vec::new(), 20)]
    #[case::none(0, Vec::new(), 20)]
    fn the_group_list_is_bounded(#[case] count: i16, #[case] groups: Vec<u32>, #[case] egid: u32) {
        let mut kinfo = kinfo();
        kinfo.kp_eproc.e_ucred.cr_ngroups = count;
        kinfo.kp_eproc.e_ucred.cr_groups =
            std::array::from_fn(|index| u32::try_from(index).unwrap_or(u32::MAX));

        assert_eq!(
            ProcessInfo::from(&kinfo),
            ProcessInfo {
                credentials: Credentials {
                    egid,
                    groups,
                    ..expected().credentials
                },
                ..expected()
            }
        );
    }

    #[test]
    fn a_full_comm_without_a_nul_is_kept() {
        let mut kinfo = kinfo();
        kinfo.kp_proc.p_comm = [b'a'.cast_signed(); ffi::MAXCOMLEN + 1];

        assert_eq!(
            ProcessInfo::from(&kinfo),
            ProcessInfo {
                comm: "a".repeat(ffi::MAXCOMLEN + 1),
                ..expected()
            }
        );
    }
}
