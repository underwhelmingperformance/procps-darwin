// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! `pgrep` and `pkill`.

mod entry;
mod error;
mod getopt;
mod mode;
mod options;
mod pidfile;
mod run;
mod table;
mod usage;

pub use entry::entry;
pub use error::{LinuxOption, PgrepError};
pub use mode::Mode;
pub use options::{Command, Options};
pub use run::main;

pub use crate::signal::LinuxSignal;
