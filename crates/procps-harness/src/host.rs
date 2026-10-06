// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::process::ExitCode;

use crate::{
    fixture::{FIXTURE_ARGUMENT, fixture_main},
    session::{LEADER_ARGUMENT, leader_main},
};

/// Runs `main`, unless a [`Runner`](crate::Runner) started this process as a
/// session leader or a fixture.
///
/// A runner starts its session leader and fixtures from copies of a host
/// binary. That binary's `main` calls this function, which recognises those
/// roles from the first argument and otherwise calls `main`.
///
/// ```no_run
/// use std::process::ExitCode;
///
/// fn main() -> ExitCode {
///     procps_harness::host(|| {
///         // Run the scenarios.
///         ExitCode::SUCCESS
///     })
/// }
/// ```
pub fn host(main: impl FnOnce() -> ExitCode) -> ExitCode {
    match std::env::args_os().nth(1) {
        Some(argument) if argument == LEADER_ARGUMENT => leader_main(),
        Some(argument) if argument == FIXTURE_ARGUMENT => fixture_main(),
        _ => main(),
    }
}
