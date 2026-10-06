// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! The helper daemon, which reads process data as root for the procps tools
//! and withholds what each caller may not read.

mod daemon;
mod deadline;
mod listener;
mod report;
mod server;
mod workers;

pub use daemon::{Config, DaemonError};
pub use listener::{ConnectionLimits, ListenError, Listener};
pub use report::ErrorChain;
pub use server::{ServeError, Server, TimeLimits};
