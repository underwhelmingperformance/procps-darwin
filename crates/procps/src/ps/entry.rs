// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

use crate::{
    output::Output,
    ps::{error::PsError, parser, run, signals, system::Host},
};

/// Runs `ps` with the process's arguments, and returns its exit status.
///
/// ```no_run
/// fn main() -> std::process::ExitCode {
///     procps::ps::entry()
/// }
/// ```
#[must_use]
pub fn entry() -> ExitCode {
    darwin_proc::restore_inherited_state();
    crate::observability::init();

    // Set the locale before parsing, as procps-ng does, because the parser
    // classifies the bytes of arguments with the locale's character classes.
    let _ = darwin_proc::set_locale(c"");

    let arguments: Vec<OsString> = std::env::args_os().collect();
    let program = crate::program_name(&arguments, "ps");
    let argument_zero = arguments.first().cloned().unwrap_or_else(|| "ps".into());

    // procps-ng ignores a failure to install its handlers, as `ps` does.
    if let Err(error) = darwin_proc::catch_signals(signals::responses(&argument_zero, &program)) {
        tracing::warn!(%error, "cannot install the signal handlers");
    }

    let parsed = parser::parse(&arguments, &Host::new());

    let mut output = Output::new();
    let (stdout, stderr) = output.streams();

    for warning in &parsed.warnings {
        // A failed write to standard error only changes the exit status, which
        // closing the output reports.
        let _ = stderr.write_all(&warning.render());
    }

    let status = match parsed.result {
        Ok(command) => run::main(&argument_zero, &program, &command, stdout, stderr),
        Err(error) => error.report(&argument_zero, &program, stderr),
    };

    match output.close() {
        Ok(()) => status,
        Err(failure) => PsError::from(failure).report(&argument_zero, &program, &mut io::stderr()),
    }
}
