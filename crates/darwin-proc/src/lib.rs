// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Safe access to the process, thread, memory and system statistics that
//! Darwin exposes through `libproc`, `sysctl` and the Mach host interfaces.

#![expect(
    unsafe_code,
    reason = "this crate wraps the libproc, sysctl and Mach calls that the tools need"
)]

mod arguments;
mod error;
mod ffi;
mod info;
mod path;
mod pid;
mod signal_set;
mod sysctl;

pub use arguments::{Arguments, MalformedArguments};
pub use error::{Call, Error};
pub use info::{Credentials, ProcessFlags, ProcessInfo, Status, Terminal};
pub use pid::Pid;
pub use signal_set::SignalSet;
