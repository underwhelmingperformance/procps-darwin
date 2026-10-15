// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::OsString, io, process::ExitCode};

use crate::{
    output::Output,
    pgrep::{Command, Mode, Options, PgrepError},
};

/// Runs `pgrep` or `pkill` with the process's arguments, and returns its exit
/// status. The program name decides which tool runs. `fallback` is the name
/// to use when the arguments are empty.
///
/// ```no_run
/// fn main() -> std::process::ExitCode {
///     procps::pgrep::entry("pgrep")
/// }
/// ```
#[must_use]
pub fn entry(fallback: &str) -> ExitCode {
    darwin_proc::restore_inherited_state();
    crate::observability::init();

    // Set the locale before parsing, as procps-ng does, because the parser
    // compares signal names with the locale's case rules, as `strcasecmp` does.
    let _ = darwin_proc::set_locale(c"");

    let arguments: Vec<OsString> = std::env::args_os().collect();
    let program = crate::program_name(&arguments, fallback);
    let mode = Mode::from_program(&program);

    let mut output = Output::with_epoll_descriptor();
    let parsed = Options::parse(mode, arguments);

    if let Ok(Command::Run(options)) = &parsed
        && options.opened_pid_file
    {
        output.open_pid_file();
    }

    let (stdout, stderr) = output.streams();

    let status = match parsed {
        Ok(command) => super::main(mode, &program, &command, stdout, stderr),
        Err(error) => error.report(mode, &program, stderr),
    };

    match output.close() {
        Ok(()) => status,
        Err(failure) => PgrepError::from(failure).report(mode, &program, &mut io::stderr()),
    }
}
