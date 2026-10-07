// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implementations of `ps`, `top`, `pgrep` and `pkill`. The binaries `pgrep`
//! and `pkill` call [`pgrep::entry`].

mod c_number;
pub mod observability;
pub mod output;
pub mod pgrep;
mod program;
mod signal;
mod strerror;

pub use program::program_name;
