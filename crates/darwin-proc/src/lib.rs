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
mod calendar;
mod catch;
mod error;
mod ffi;
mod files;
mod info;
mod kill;
mod libproc;
mod locale;
mod path;
mod pid;
mod regex;
mod region;
mod resource;
mod signal_set;
mod socket;
mod startup;
mod strerror;
mod sysctl;
mod system;
mod task;
mod terminal;
mod thread;
mod time;
mod user;

pub use arguments::{Arguments, MalformedArguments};
pub use calendar::CalendarTime;
pub use catch::{SignalResponse, catch_signals, unblock_signal};
pub use error::{Call, Error};
pub use info::{AccountingFlags, Credentials, ProcessFlags, ProcessInfo, Status, Terminal};
pub use locale::{CaseFolding, Locale, decode_each_byte, set_locale};
pub use pid::Pid;
pub use regex::{Regex, RegexError, RegexErrorKind};
pub use region::{Protection, Region, RegionWalk, ShareMode};
pub use resource::{ResourceCounters, ResourceUsage};
pub use signal_set::SignalSet;
pub use socket::{LaunchdSockets, Peer, PeerCredentials};
pub use startup::{ClosedDescriptors, ClosedStreams, StandardStream, restore_inherited_state};
pub use strerror::strerror;
pub use system::{Host, LoadAverage, Memory, ProcessorTicks, Swap, TaskTotals};
pub use task::{SchedulingPolicy, TaskInfo};
pub use thread::{RunState, ThreadInfo};
pub use user::{Gid, Uid};
