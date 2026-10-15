// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Implementations of `ps`, `top`, `pgrep` and `pkill`. The binaries `pgrep`
//! and `pkill` call [`pgrep::entry`], and `ps` calls [`ps::entry`].

mod c_number;
mod linux;
pub mod observability;
pub mod output;
pub mod pgrep;
mod program;
pub mod ps;
mod signal;
mod strerror;

pub use program::program_name;
