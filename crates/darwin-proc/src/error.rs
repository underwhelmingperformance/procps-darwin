// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io;

use crate::Pid;

/// A call that reads process data from the kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq, derive_more::Display)]
#[non_exhaustive]
pub enum Call {
    /// The `kern.proc` sysctl, which returns `struct kinfo_proc`.
    #[display("sysctl kern.proc")]
    ProcessTable,
    /// `proc_pidpath`.
    #[display("proc_pidpath")]
    ExecutablePath,
    /// `getsid`.
    #[display("getsid")]
    Session,
    /// The `kern.procargs2` sysctl, which returns the arguments and
    /// environment.
    #[display("sysctl kern.procargs2")]
    Arguments,
    /// `proc_pidinfo` with `PROC_PIDTASKINFO`.
    #[display("proc_pidinfo PROC_PIDTASKINFO")]
    TaskInfo,
    /// `proc_pid_rusage` with `RUSAGE_INFO_V6`.
    #[display("proc_pid_rusage RUSAGE_INFO_V6")]
    ResourceUsage,
    /// `proc_pidinfo` with `PROC_PIDLISTTHREADIDS`.
    #[display("proc_pidinfo PROC_PIDLISTTHREADIDS")]
    ThreadList,
    /// `proc_pidinfo` with `PROC_PIDTHREADID64INFO`.
    #[display("proc_pidinfo PROC_PIDTHREADID64INFO")]
    ThreadInfo,
    /// `proc_pidinfo` with `PROC_PIDREGIONINFO`.
    #[display("proc_pidinfo PROC_PIDREGIONINFO")]
    Regions,
}

/// An error from reading process data.
///
/// The tools show each case differently. They leave an exited process out of
/// their output, and show `-` for a denied read. Data that the kernel does not
/// provide can have its own form: procps-ng shows a process without arguments
/// as `[comm]`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The process does not exist, usually because it has exited.
    #[error("process {pid} has exited")]
    Exited {
        /// The process.
        pid: Pid,
    },
    /// The caller may not read this data for this process.
    #[error("{call} for process {pid}: permission denied")]
    Denied {
        /// The process.
        pid: Pid,
        /// The call that failed.
        call: Call,
    },
    /// The process exists, but the kernel does not provide this data for it.
    /// `kernel_task` has no executable path, for example.
    #[error("{call} does not provide data for process {pid}")]
    Unsupported {
        /// The process.
        pid: Pid,
        /// The call that failed.
        call: Call,
    },
    /// The call failed for another reason.
    #[error("{call}: {source}")]
    Os {
        /// The call that failed.
        call: Call,
        /// The underlying error.
        source: io::Error,
    },
}

impl Error {
    /// The error for `source`, which `call` returned for `pid`.
    pub(crate) fn for_process(call: Call, pid: Pid, source: io::Error) -> Self {
        match source.raw_os_error() {
            Some(libc::ESRCH) => Self::Exited { pid },
            Some(libc::EPERM | libc::EACCES) => Self::Denied { pid, call },
            _ => Self::Os { call, source },
        }
    }
}

impl Pid {
    /// The error for `source`, which `call` returned for this process.
    ///
    /// Several calls fail with `ESRCH` for a zombie as well as for a process
    /// that has exited, and `proc_pidpath` also fails with it for
    /// `kernel_task`. This function therefore calls [`Pid::info`]: a process
    /// that is still listed lacks the data and has not exited.
    pub(crate) fn live_error(self, call: Call, source: io::Error) -> Error {
        let error = Error::for_process(call, self, source);

        if matches!(error, Error::Exited { .. }) && self.info().is_ok() {
            return Error::Unsupported { pid: self, call };
        }

        error
    }
}
