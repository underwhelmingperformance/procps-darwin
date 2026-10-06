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
