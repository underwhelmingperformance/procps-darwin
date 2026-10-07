// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::OsStr, io::Write, os::unix::ffi::OsStrExt, process::ExitCode};

use crate::pgrep::{Command, Mode, PgrepError, usage};

/// The version of procps-ng that the tools reproduce.
const PROCPS_NG_VERSION: &str = "4.0.7";

/// Runs `command` for the tool that `mode` selects, invoked as `program`, and
/// returns the exit status.
///
/// ```
/// use std::process::ExitCode;
///
/// use procps::pgrep::{Command, Mode};
///
/// let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
/// let status = procps::pgrep::main(
///     Mode::Pgrep,
///     "pgrep".as_ref(),
///     &Command::Version,
///     &mut stdout,
///     &mut stderr,
/// );
///
/// assert_eq!(
///     (String::from_utf8(stdout)?, status),
///     (
///         format!(
///             "pgrep from procps-darwin {} (procps-ng 4.0.7)\n",
///             env!("CARGO_PKG_VERSION")
///         ),
///         ExitCode::SUCCESS
///     )
/// );
/// # Ok::<(), std::string::FromUtf8Error>(())
/// ```
#[must_use]
pub fn main(
    mode: Mode,
    program: &OsStr,
    command: &Command,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitCode {
    // A failed write is reported when the tool closes its output, as
    // procps-ng's `close_stdout` reports it.
    let _ = match command {
        Command::Help => stdout.write_all(&usage::usage(mode, program)),
        Command::Version => stdout.write_all(
            &[
                program.as_bytes(),
                format!(
                    " from procps-darwin {} (procps-ng {PROCPS_NG_VERSION})\n",
                    env!("CARGO_PKG_VERSION")
                )
                .as_bytes(),
            ]
            .concat(),
        ),
        Command::Run(_) => return PgrepError::Unimplemented.report(mode, program, stderr),
    };

    ExitCode::SUCCESS
}
