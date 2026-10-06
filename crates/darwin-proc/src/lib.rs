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
mod files;
mod info;
mod libproc;
mod path;
mod pid;
mod region;
mod resource;
mod signal_set;
mod sysctl;
mod system;
mod task;
mod terminal;
mod thread;
mod time;
mod user;

pub use arguments::{Arguments, MalformedArguments};
pub use error::{Call, Error};
pub use info::{AccountingFlags, Credentials, ProcessFlags, ProcessInfo, Status, Terminal};
pub use pid::Pid;
pub use region::{Protection, Region, ShareMode};
pub use resource::ResourceUsage;
pub use signal_set::SignalSet;
pub use system::{Host, LoadAverage, Memory, ProcessorTicks, Swap, TaskTotals};
pub use task::{SchedulingPolicy, TaskInfo};
pub use thread::{RunState, ThreadInfo};
pub use user::{Gid, Uid};
