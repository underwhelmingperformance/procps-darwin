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

use std::{ffi::OsStr, io::Write, os::unix::ffi::OsStrExt, process::ExitCode};

use darwin_proc::{Locale, Pid};
use procps_core::{Charset, ProcessSource, SourceChoice};

use crate::pgrep::{
    Command, Mode, Options, PgrepError,
    options::{Listing, Report, Tasks},
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
/// status if the snapshot fails. `pkill`, and `pgrep` with `-l`, `-a`, `-Q` or
/// `-w`, exit with status 3 before the snapshot, because `pkill`'s signals and
/// these output options are not implemented yet.
pub(crate) fn run(
    invocation: &Invocation<'_>,
    selection: &Selection<'_>,
    source: &impl ProcessSource,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitCode {
    let Invocation { mode, program, .. } = *invocation;
    let options = selection.options();

    let unimplemented = [
        (mode == Mode::Pkill, "signalling"),
        (options.output.report == Report::List(Listing::Names), "-l"),
        (
            options.output.report == Report::List(Listing::CommandLines),
            "-a",
        ),
        (options.output.shell_quote, "-Q"),
        (options.tasks == Tasks::Threads, "-w"),
    ]
    .into_iter()
    .find_map(|(used, feature)| used.then_some(feature));

    if let Some(feature) = unimplemented {
        return PgrepError::Unimplemented(feature).report(mode, program, stderr);
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
/// `selected` processes, or their pids.
///
/// procps-ng prints each line with one `printf` call, and goes on after a call
/// fails. When a flush fails, the stream discards what glibc discards: the
/// buffered bytes and the rest of the line. The streams record each failure,
/// and the tool reports it when it closes its output.
fn report(
    invocation: &Invocation<'_>,
    options: &Options,
    selected: &[&procps_core::Process],
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

    for (index, process) in selected.iter().enumerate() {
        let delimiter = if index + 1 == selected.len() {
            b"\n"
        } else {
            options.output.delimiter.as_bytes()
        };
        let line = [process.info.pid.to_string().as_bytes(), delimiter].concat();

        let _ = stdout.write_all(&line);
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

    use darwin_proc::{
        AccountingFlags, Credentials, Gid, Pid, ProcessFlags, ProcessInfo, SignalSet, Status, Uid,
    };
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, FixtureSource, Process, ProcessSource, Snapshot, SnapshotRequest,
        SourceError,
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

    fn process(pid: i32, comm: &str) -> Process {
        let uid = Uid::from(501);
        let gid = Gid::from(20);
        let info = ProcessInfo {
            pid: Pid::from(pid),
            ppid: Pid::from(1),
            pgid: Pid::from(pid),
            terminal: None,
            credentials: Credentials {
                ruid: uid,
                euid: uid,
                svuid: uid,
                rgid: gid,
                egid: gid,
                svgid: gid,
                groups: vec![gid],
            },
            nice: 0,
            priority: 31,
            status: Status::Running,
            flags: ProcessFlags::default(),
            accounting: AccountingFlags::default(),
            session_leader: false,
            start_time: SystemTime::UNIX_EPOCH,
            comm: comm.into(),
            ignored_signals: SignalSet::default(),
            caught_signals: SignalSet::default(),
        };

        Process::from_identity(info, Field::Unsupported, Field::Unsupported)
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
        let source = FixtureSource::new(
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
            vec![process(10, "sleep"), process(20, "sleep")],
        );

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
    #[case::unimplemented(
        &["pgrep", "-l", "sleep"],
        "",
        "pgrep: -l is not implemented yet\n",
        ExitCode::from(3)
    )]
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

    // procps-ng writes each pid and its delimiter with one `printf` call, and
    // goes on after a call fails.
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
        let selected: Vec<&Process> = processes.iter().collect();
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
}
