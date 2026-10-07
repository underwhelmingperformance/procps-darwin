// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2009-2026 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2013-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 2012 Roberto Polli <rpolli@babel.it>
// SPDX-FileCopyrightText: 2002-2007 Albert Cahalan
// SPDX-FileCopyrightText: 2000 Kjetil Torgrim Homme <kjetilho@ifi.uio.no>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/pgrep.c.

use std::{
    ffi::OsStr,
    io::Write,
    os::unix::ffi::{OsStrExt, OsStringExt},
    process::ExitCode,
};

use darwin_proc::{Locale, Pid};
use procps_core::{Charset, LinuxTask, ProcessSource, SourceChoice};

use crate::pgrep::{
    Command, Mode, Options, PgrepError,
    options::{Listing, Report},
    quote::shell_quote,
    select::{self, Invocation, Selection, is_long_match},
    usage,
};

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
        Command::Run(options) => {
            let locale = Locale::current();
            let selection = match Selection::new(options) {
                Ok(selection) => selection,
                Err(error) => return error.report(mode, program, stderr),
            };
            let invocation = Invocation {
                mode,
                program,
                myself: Pid::current(),
                charset: Charset::from_codeset(locale.codeset()),
            };

            return match SourceChoice::current().choose() {
                Ok(source) => run(&invocation, &selection, &source, stdout, stderr),
                Err(error) => PgrepError::from(error).report(mode, program, stderr),
            };
        }
    };

    ExitCode::SUCCESS
}

/// Takes a snapshot from `source`, selects the processes in it that
/// `selection` describes, and reports them as procps-ng's `pgrep` does.
/// Returns status 0 if any process matches, 1 if none does, and an error's
/// status if the snapshot fails. `pkill` exits with status 3 before the
/// snapshot, because it cannot send signals yet.
pub(crate) fn run(
    invocation: &Invocation<'_>,
    selection: &Selection<'_>,
    source: &impl ProcessSource,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitCode {
    let Invocation { mode, program, .. } = *invocation;
    let options = selection.options();

    if mode == Mode::Pkill {
        return PgrepError::Unimplemented.report(mode, program, stderr);
    }

    let snapshot = match source.snapshot(&select::request(options)) {
        Ok(snapshot) => snapshot,
        Err(error) => return PgrepError::from(error).report(mode, program, stderr),
    };
    let selected = selection.select(&snapshot, invocation);

    report(invocation, options, &selected, stdout, stderr);

    if selected.is_empty() {
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}

/// Writes procps-ng's warning about a long pattern, then the count of the
/// `selected` tasks, or their IDs, with their names or command lines for `-l`
/// or `-a`.
///
/// procps-ng prints each line with one `printf` call, and goes on after a call
/// fails. When a flush fails, the stream discards what glibc discards: the
/// buffered bytes and the rest of the line. The streams record each failure,
/// and the tool reports it when it closes its output.
fn report(
    invocation: &Invocation<'_>,
    options: &Options,
    selected: &[LinuxTask<'_>],
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) {
    let program = invocation.program.as_bytes();

    if selected.is_empty()
        && !options.matching.full
        && options.pattern.as_deref().is_some_and(is_long_match)
    {
        let _ = stderr.write_all(
            &[
                program,
                b": pattern that searches for process name longer than 15 characters will result \
                  in zero matches\nTry `",
                program,
                b" -f' option to match against the complete command line.\n",
            ]
            .concat(),
        );
    }

    match options.output.report {
        Report::Count => {
            let _ = stdout.write_all(format!("{}\n", selected.len()).as_bytes());
            return;
        }
        Report::Quiet => return,
        Report::List(_) => {}
    }

    let listed = matches!(
        options.output.report,
        Report::List(Listing::Names | Listing::CommandLines)
    );

    for (index, task) in selected.iter().enumerate() {
        let delimiter = if index + 1 == selected.len() {
            b"\n"
        } else {
            options.output.delimiter.as_bytes()
        };
        let mut line = task.id().to_string().into_bytes();

        if listed {
            line.push(b' ');
            line.extend_from_slice(&text(options, task, invocation.charset));
        }

        line.extend_from_slice(delimiter);
        let _ = stdout.write_all(&line);
    }
}

/// The longest text that procps-ng prints after a task's ID. procps-ng's
/// buffer has `get_arg_max()` bytes. `get_arg_max` caps `sysconf(_SC_ARG_MAX)`
/// at 128 KiB, and glibc's `sysconf(_SC_ARG_MAX)` on Linux is at least 128
/// KiB, so the buffer has 128 KiB on Linux. `strncpy` leaves the last byte for
/// the NUL. procps-core cuts each argument of `CMDLINE_V` to the same length,
/// but that limit comes from the buffers of procps-ng's library, not from
/// `pgrep`, so either value can change without the other.
const TEXT_LENGTH: usize = 128 * 1024 - 1;

/// The text that procps-ng prints after a task's ID, cut to the length of
/// procps-ng's buffer. Quoting can make the text longer than the buffer, and
/// procps-ng cuts it after byte 131071, even inside a multi-byte character.
fn text(options: &Options, task: &LinuxTask<'_>, charset: Charset) -> Vec<u8> {
    let mut text = full_text(options, task, charset);
    text.truncate(TEXT_LENGTH);

    text
}

/// The command line for `-a`, or the name otherwise, quoted for the shell
/// with `-Q`.
fn full_text(options: &Options, task: &LinuxTask<'_>, charset: Charset) -> Vec<u8> {
    let command_line = options.output.report == Report::List(Listing::CommandLines);

    match (options.output.shell_quote, command_line) {
        (true, true) => shell_quote(
            task.process()
                .linux_command_line_vector(charset)
                .iter()
                .map(|argument| argument.as_bytes()),
        ),
        (true, false) => shell_quote([task.command(charset).as_bytes()]),
        (false, true) => task.command_line(charset).into_vec(),
        (false, false) => task.command(charset).into_vec(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        io::{self, Write},
        process::ExitCode,
        time::{Duration, SystemTime},
    };

    use darwin_proc::{Pid, RunState};
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, FixtureSource, Process, ProcessSource, Snapshot, SnapshotRequest,
        SourceError,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{report, run};
    use crate::pgrep::{
        Command, Mode, Options,
        select::{Invocation, Selection},
    };

    /// A writer whose writes fail, as standard error does when it is open
    /// only for reading.
    struct Failing;

    impl Write for Failing {
        fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(libc::EBADF))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A source whose every snapshot fails.
    struct TimingOut;

    impl ProcessSource for TimingOut {
        fn snapshot(&self, _request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
            Err(SourceError::TimedOut)
        }
    }

    /// The standard output and exit status of `pgrep` with `arguments`, for
    /// two processes called `sleep`. The tool writes its standard error to
    /// `stderr`.
    fn pgrep(
        arguments: &[&str],
        stderr: &mut impl Write,
    ) -> Result<(String, ExitCode), Box<dyn std::error::Error>> {
        pgrep_in(
            vec![process(10, "sleep"), process(20, "sleep")],
            arguments,
            stderr,
        )
    }

    #[test]
    fn a_long_text_is_cut_where_procps_ng_cuts_it() -> Result<(), Box<dyn std::error::Error>> {
        let mut process = process(10, "sh");
        process.arguments = Some(Field::Available(vec!["'".repeat(40_000).into()]));
        let quoted = format!("'{}'", "'\\''".repeat(40_000));

        assert_eq!(
            pgrep_output(vec![process], &["pgrep", "-a", "-Q", "sh"])?,
            (
                format!("10 {}\n", &quoted[..131_071]),
                String::new(),
                ExitCode::SUCCESS
            )
        );

        Ok(())
    }

    // procps-ng's fallback for a command line that it cannot read uses each
    // task's own name.
    #[test]
    fn a_thread_without_readable_arguments_is_listed_by_its_own_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut process = process(30, "x");
        process.arguments = Some(Field::Denied);
        process.threads = Some(Field::Available(vec![
            thread(300, b"", RunState::Waiting),
            thread(301, b"io", RunState::Waiting),
        ]));

        assert_eq!(
            pgrep_output(vec![process], &["pgrep", "-w", "-a", "."])?,
            (
                "300 [x]\n301 [io]\n".to_owned(),
                String::new(),
                ExitCode::SUCCESS
            )
        );

        Ok(())
    }

    #[test]
    fn denied_arguments_are_quoted_as_a_dash() -> Result<(), Box<dyn std::error::Error>> {
        let mut process = process(30, "x");
        process.arguments = Some(Field::Denied);

        assert_eq!(
            pgrep_output(vec![process], &["pgrep", "-a", "-Q", "x"])?,
            ("30 -\n".to_owned(), String::new(), ExitCode::SUCCESS)
        );

        Ok(())
    }

    /// The standard output, standard error and exit status of `pgrep` with
    /// `arguments`, for two processes with arguments. The first has two
    /// threads, and the source could not read the threads of the second.
    fn pgrep_listing(
        arguments: &[&str],
    ) -> Result<(String, String, ExitCode), Box<dyn std::error::Error>> {
        let mut first = process(10, "sleep");
        first.arguments = Some(Field::Available(vec!["sleep".into(), "10".into()]));
        first.threads = Some(Field::Available(vec![
            thread(901, b"worker", RunState::Waiting),
            thread(900, b"main", RunState::Waiting),
        ]));
        let mut second = process(20, "it's");
        second.arguments = Some(Field::Available(vec![
            "sh".into(),
            "-c".into(),
            "echo 'hi'".into(),
        ]));
        second.threads = Some(Field::Denied);

        pgrep_output(vec![first, second], arguments)
    }

    /// The standard output, standard error and exit status of `pgrep` with
    /// `arguments`, for `processes`.
    fn pgrep_output(
        processes: Vec<Process>,
        arguments: &[&str],
    ) -> Result<(String, String, ExitCode), Box<dyn std::error::Error>> {
        let mut stderr = Vec::new();
        let (stdout, status) = pgrep_in(processes, arguments, &mut stderr)?;

        Ok((stdout, String::from_utf8(stderr)?, status))
    }

    fn pgrep_in(
        processes: Vec<Process>,
        arguments: &[&str],
        stderr: &mut impl Write,
    ) -> Result<(String, ExitCode), Box<dyn std::error::Error>> {
        let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, processes);

        pgrep_from(&source, arguments, stderr)
    }

    /// The standard output and exit status of `pgrep` with `arguments`, for
    /// the processes in `source`.
    fn pgrep_from(
        source: &impl ProcessSource,
        arguments: &[&str],
        stderr: &mut impl Write,
    ) -> Result<(String, ExitCode), Box<dyn std::error::Error>> {
        let Command::Run(options) =
            Options::parse(Mode::Pgrep, arguments.iter().map(OsString::from))?
        else {
            return Err("expected options".into());
        };
        let invocation = Invocation {
            mode: Mode::Pgrep,
            program: OsStr::new("pgrep"),
            myself: Pid::from(1),
            charset: Charset::Utf8,
        };
        let selection = Selection::new(&options)?;
        let mut stdout = Vec::new();
        let status = run(&invocation, &selection, source, &mut stdout, stderr);

        Ok((String::from_utf8(stdout)?, status))
    }

    #[rstest]
    #[case::pids(&["pgrep", "sleep"], "10\n20\n", "", ExitCode::SUCCESS)]
    #[case::delimiter(&["pgrep", "-d", ", ", "sleep"], "10, 20\n", "", ExitCode::SUCCESS)]
    #[case::count(&["pgrep", "-c", "sleep"], "2\n", "", ExitCode::SUCCESS)]
    #[case::count_of_none(&["pgrep", "-c", "nothing"], "0\n", "", ExitCode::from(1))]
    #[case::quiet(&["pgrep", "--quiet", "sleep"], "", "", ExitCode::SUCCESS)]
    #[case::no_match(&["pgrep", "nothing"], "", "", ExitCode::from(1))]
    #[case::names(&["pgrep", "-l", "sleep"], "10 sleep\n20 sleep\n", "", ExitCode::SUCCESS)]
    #[case::long_pattern(
        &["pgrep", "sleep-for-a-long-time"],
        "",
        "pgrep: pattern that searches for process name longer than 15 characters will result \
         in zero matches\nTry `pgrep -f' option to match against the complete command line.\n",
        ExitCode::from(1)
    )]
    fn the_selected_processes_are_reported_as_procps_ng_reports_them(
        #[case] arguments: &[&str],
        #[case] stdout: &str,
        #[case] stderr: &str,
        #[case] status: ExitCode,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut written = Vec::new();
        let (output, code) = pgrep(arguments, &mut written)?;

        assert_eq!(
            (output, String::from_utf8(written)?, code),
            (stdout.to_owned(), stderr.to_owned(), status)
        );

        Ok(())
    }

    #[test]
    fn a_failed_snapshot_is_a_fatal_error() -> Result<(), Box<dyn std::error::Error>> {
        let mut stderr = Vec::new();
        let (stdout, status) = pgrep_from(&TimingOut, &["pgrep", "sleep"], &mut stderr)?;

        assert_eq!(
            (stdout, String::from_utf8(stderr)?, status),
            (
                String::new(),
                "pgrep: the snapshot reached its time limit\n".to_owned(),
                ExitCode::from(3)
            )
        );

        Ok(())
    }

    #[test]
    fn a_failed_warning_leaves_the_count() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            pgrep(&["pgrep", "-c", "sleep-for-a-long-time"], &mut Failing)?,
            ("0\n".to_owned(), ExitCode::from(1))
        );

        Ok(())
    }

    /// A writer whose first write fails, like the flush of a full buffer,
    /// and which keeps the bytes of each later write.
    #[derive(Default)]
    struct FailingOnce {
        writes: Vec<Vec<u8>>,
        failed: bool,
    }

    impl Write for FailingOnce {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.failed {
                self.failed = true;
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }

            self.writes.push(bytes.to_vec());

            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // procps-ng writes each line with one `printf` call, and goes on after a
    // call fails.
    #[test]
    fn report_goes_on_writing_after_a_failed_write() -> Result<(), Box<dyn std::error::Error>> {
        let Command::Run(options) = Options::parse(
            Mode::Pgrep,
            ["pgrep", "-d", ", ", "sleep"].map(OsString::from),
        )?
        else {
            return Err("expected options".into());
        };
        let processes = [
            process(10, "sleep"),
            process(20, "sleep"),
            process(30, "sleep"),
        ];
        let selected: Vec<_> = processes.iter().map(Process::linux_task).collect();
        let invocation = Invocation {
            mode: Mode::Pgrep,
            program: OsStr::new("pgrep"),
            myself: Pid::from(1),
            charset: Charset::Utf8,
        };
        let mut stdout = FailingOnce::default();

        report(
            &invocation,
            &options,
            &selected,
            &mut stdout,
            &mut Vec::new(),
        );

        assert_eq!(stdout.writes, [b"20, ".to_vec(), b"30\n".to_vec()]);

        Ok(())
    }

    #[rstest]
    #[case::names(&["pgrep", "-l", "."], "10 sleep\n20 it's\n")]
    #[case::command_lines(&["pgrep", "-a", "."], "10 sleep 10\n20 sh -c echo 'hi'\n")]
    #[case::both(&["pgrep", "-l", "-a", "."], "10 sleep 10\n20 sh -c echo 'hi'\n")]
    #[case::quoted_names(&["pgrep", "-l", "-Q", "."], "10 sleep\n20 'it'\\''s'\n")]
    #[case::quoted_command_lines(
        &["pgrep", "-a", "-Q", "."],
        "10 sleep 10\n20 sh -c 'echo '\\''hi'\\'''\n"
    )]
    #[case::quoted_pids(&["pgrep", "-Q", "."], "10\n20\n")]
    #[case::delimiter(&["pgrep", "-l", "-d", ", ", "."], "10 sleep, 20 it's\n")]
    #[case::threads(&["pgrep", "-w", "-l", "."], "900 sleep\n901 worker\n20 it's\n")]
    #[case::thread_names(&["pgrep", "-w", "work"], "901\n")]
    #[case::main_thread(&["pgrep", "-w", "^sleep$"], "900\n")]
    #[case::count(&["pgrep", "-c", "-l", "."], "2\n")]
    #[case::thread_command_lines(&["pgrep", "-w", "-a", "."], "900 sleep 10\n901 sleep 10\n20 sh -c echo 'hi'\n")]
    #[case::quoted_thread_names(&["pgrep", "-w", "-l", "-Q", "."], "900 sleep\n901 worker\n20 'it'\\''s'\n")]
    #[case::inverse_threads(&["pgrep", "-w", "-v", "work"], "900\n20\n")]
    fn the_selected_tasks_are_listed_as_procps_ng_lists_them(
        #[case] arguments: &[&str],
        #[case] expected: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            pgrep_listing(arguments)?,
            (expected.to_owned(), String::new(), ExitCode::SUCCESS)
        );

        Ok(())
    }
}
