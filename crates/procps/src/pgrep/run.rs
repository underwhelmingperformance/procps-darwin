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
    io::{self, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    process::ExitCode,
    time::SystemTime,
};

use darwin_proc::{Locale, Pid};
use procps_core::{Charset, LinuxTask, ProcessSource, SourceChoice};

use crate::{
    pgrep::{
        Command, Mode, Options, PgrepError,
        options::{Listing, Report},
        quote::shell_quote,
        select::{self, Invocation, Selected, Selection, is_long_match},
        usage,
    },
    strerror::strerror,
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
                Ok(source) => run(&invocation, &selection, &source, &Kernel, stdout, stderr),
                Err(error) => PgrepError::from(error).report(mode, program, stderr),
            };
        }
    };

    ExitCode::SUCCESS
}

/// The kernel calls that `pkill` makes for each selected process: reading
/// its start time and sending it a signal. Tests replace the kernel's calls.
pub(crate) trait Signaller {
    /// The start time of the process that has `pid` now.
    fn start_time(&self, pid: Pid) -> Result<SystemTime, darwin_proc::Error>;

    /// Sends the signal numbered `signal` to the process `pid`.
    fn send(&self, pid: Pid, signal: libc::c_int) -> Result<(), darwin_proc::Error>;
}

/// The kernel, which signals processes with `kill(2)` as the invoking user.
pub(crate) struct Kernel;

impl Signaller for Kernel {
    fn start_time(&self, pid: Pid) -> Result<SystemTime, darwin_proc::Error> {
        Ok(pid.info()?.start_time)
    }

    fn send(&self, pid: Pid, signal: libc::c_int) -> Result<(), darwin_proc::Error> {
        pid.send_signal(signal)
    }
}

/// Takes a snapshot from `source` and selects the processes in it that
/// `selection` describes. `pgrep` then reports them, and `pkill` signals them
/// through `signaller`, as procps-ng does. Returns the exit status: an
/// error's status if the snapshot fails, 0 if `pgrep` selects a process or
/// `pkill` signals one, and 1 otherwise.
pub(crate) fn run(
    invocation: &Invocation<'_>,
    selection: &Selection<'_>,
    source: &impl ProcessSource,
    signaller: &impl Signaller,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitCode {
    let Invocation { mode, program, .. } = *invocation;
    let options = selection.options();

    let snapshot = match source.snapshot(&select::request(options)) {
        Ok(snapshot) => snapshot,
        Err(error) => return PgrepError::from(error).report(mode, program, stderr),
    };
    let selected = selection.select(&snapshot, invocation);

    // A failed write must not stop the output: the stream records the
    // failure, and the tool reports it when it closes its output, as
    // procps-ng does.
    let _ = warn(invocation, options, &selected, stderr);

    if mode == Mode::Pkill {
        return signal(invocation, options, &selected, signaller, stdout, stderr);
    }

    report(invocation, options, &selected, stdout);

    if selected.is_empty() {
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}

/// Signals the `selected` tasks as procps-ng's `pkill` does, and returns the
/// exit status: 0 if `kill(2)` succeeded for any task, and 1 otherwise.
///
/// procps-ng opens a pidfd for each pid after the selection. The pidfd refers
/// to whichever process has the pid by then, so procps-ng can signal a process
/// that took the pid after the selection. Darwin has no pidfds. To avoid
/// signalling such a process, this function reads each process's start time
/// again just before it sends the signal. If the process has exited, or a
/// different start time shows that another process has the pid, the function
/// leaves the process out. If the read fails for another reason, the function
/// reports the failure in the same form as a failed signal. With
/// `--lightweight`, each thread's process receives the signal, because Darwin's
/// thread IDs are not pids.
fn signal(
    invocation: &Invocation<'_>,
    options: &Options,
    selected: &[Selected<'_>],
    signaller: &impl Signaller,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> ExitCode {
    let program = invocation.program.as_bytes();
    let mut delivered = 0_usize;
    let failed = |stderr: &mut dyn Write, id: u64, error: &darwin_proc::Error| {
        let id = id.to_string();
        let description = description(error);
        let _ = stderr.write_all(
            &[
                program,
                b": killing pid ",
                id.as_bytes(),
                b" failed: ",
                description.as_bytes(),
                b"\n",
            ]
            .concat(),
        );
    };

    for Selected { task, output } in selected {
        let pid = task.process().info.pid;

        match signaller.start_time(pid) {
            Ok(start) if start == task.process().info.start_time => {}
            Ok(_) | Err(darwin_proc::Error::Exited { .. }) => {
                tracing::debug!(%pid, "left out a process that has exited or whose start time changed");
                continue;
            }
            Err(error) => {
                failed(stderr, task.id(), &error);
                continue;
            }
        }

        match signaller.send(pid, options.signal) {
            Ok(()) => {
                delivered += 1;

                if options.output.echo {
                    let text = output
                        .map(|output| text(options, &output, invocation.charset))
                        .unwrap_or_default();
                    let echo =
                        [text, format!(" killed (pid {})\n", task.id()).into_bytes()].concat();
                    let _ = stdout.write_all(&echo);
                }
            }
            Err(darwin_proc::Error::Exited { .. }) => {}
            Err(error) => failed(stderr, task.id(), &error),
        }
    }

    if options.output.report == Report::Count {
        let _ = stdout.write_all(format!("{}\n", selected.len()).as_bytes());
    }

    if delivered == 0 {
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}

/// The description that `pkill` prints for a failed signal or a failed read of
/// a start time: glibc's description of the error number, as procps-ng prints
/// for a failed signal. A denied call has the description of `EPERM`, the error
/// from `kill(2)` for a process that the caller may not signal. An error
/// without an error number has darwin-proc's description.
fn description(error: &darwin_proc::Error) -> String {
    match error {
        darwin_proc::Error::Denied { .. } => strerror(libc::EPERM),
        darwin_proc::Error::Os { source, .. } => source
            .raw_os_error()
            .map_or_else(|| source.to_string(), strerror),
        _ => error.to_string(),
    }
}

/// Writes procps-ng's warning that a pattern cannot match a command name,
/// when nothing matches, `-f` is not given, and [`is_long_match`] accepts the
/// pattern.
fn warn(
    invocation: &Invocation<'_>,
    options: &Options,
    selected: &[Selected<'_>],
    stderr: &mut impl Write,
) -> io::Result<()> {
    let program = invocation.program.as_bytes();

    if !selected.is_empty()
        || options.matching.full
        || !options.pattern.as_deref().is_some_and(is_long_match)
    {
        return Ok(());
    }

    stderr.write_all(
        &[
            program,
            b": pattern that searches for process name longer than 15 characters will result \
              in zero matches\nTry `",
            program,
            b" -f' option to match against the complete command line.\n",
        ]
        .concat(),
    )
}

/// Writes the count of the `selected` tasks, or their IDs, with their names
/// or command lines for `-l` or `-a`, as procps-ng's `pgrep` does.
///
/// procps-ng prints each line with one `printf` call, and goes on after a call
/// fails. When a flush fails, the stream discards what glibc discards: the
/// buffered bytes and the rest of the line. The stream records each failure,
/// and the tool reports it when it closes its output.
fn report(
    invocation: &Invocation<'_>,
    options: &Options,
    selected: &[Selected<'_>],
    stdout: &mut impl Write,
) {
    match options.output.report {
        Report::Count => {
            let _ = stdout.write_all(format!("{}\n", selected.len()).as_bytes());
            return;
        }
        Report::Quiet => return,
        Report::List => {}
    }

    let listed = options.output.listing != Listing::Ids;

    for (index, Selected { task, .. }) in selected.iter().enumerate() {
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
    let command_line = options.output.listing == Listing::CommandLines;

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
        cell::RefCell,
        ffi::{OsStr, OsString},
        io::{self, Write},
        process::ExitCode,
        time::{Duration, SystemTime},
    };

    use darwin_proc::{Call, Pid, RunState};
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, FixtureSource, Process, ProcessSource, Snapshot, SnapshotRequest,
        SourceError,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{Signaller, report, run};
    use crate::pgrep::{
        Command, Mode, Options,
        select::{Invocation, Selected, Selection},
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

    /// How a fake's kernel call fails.
    #[derive(Clone, Copy, Debug)]
    enum Failure {
        Exited,
        Denied,
        Os(libc::c_int),
        Unsupported,
    }

    impl Failure {
        fn error(self, pid: Pid, call: Call) -> darwin_proc::Error {
            match self {
                Self::Exited => darwin_proc::Error::Exited { pid },
                Self::Denied => darwin_proc::Error::Denied { pid, call },
                Self::Os(errno) => darwin_proc::Error::Os {
                    call,
                    source: io::Error::from_raw_os_error(errno),
                },
                Self::Unsupported => darwin_proc::Error::Unsupported { pid, call },
            }
        }
    }

    /// What a fake reads again for a process's start time.
    #[derive(Clone, Copy, Debug)]
    enum Reread {
        Started(SystemTime),
        Failed(Failure),
    }

    /// A fake [`Signaller`] for `pkill`'s tests. It records each signal that it
    /// delivers, fails to signal the processes in `failures`, and reads the
    /// start times in `rereads` in place of the snapshot's.
    #[derive(Default)]
    struct FakeSignaller {
        sent: RefCell<Vec<(Pid, libc::c_int)>>,
        failures: Vec<(Pid, Failure)>,
        rereads: Vec<(Pid, Reread)>,
    }

    impl Signaller for FakeSignaller {
        fn start_time(&self, pid: Pid) -> Result<SystemTime, darwin_proc::Error> {
            let reread = self
                .rereads
                .iter()
                .find(|&&(reread, _)| reread == pid)
                .map_or(Reread::Started(SystemTime::UNIX_EPOCH), |&(_, reread)| {
                    reread
                });

            match reread {
                Reread::Started(start) => Ok(start),
                Reread::Failed(failure) => Err(failure.error(pid, Call::ProcessTable)),
            }
        }

        fn send(&self, pid: Pid, signal: libc::c_int) -> Result<(), darwin_proc::Error> {
            if let Some(&(_, failure)) = self.failures.iter().find(|&&(failing, _)| failing == pid)
            {
                return Err(failure.error(pid, Call::Signal));
            }

            self.sent.borrow_mut().push((pid, signal));

            Ok(())
        }
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
        run_tool(
            Mode::Pgrep,
            source,
            arguments,
            &FakeSignaller::default(),
            stderr,
        )
    }

    /// The standard output and exit status of the tool in `mode` with
    /// `arguments`, for the processes in `source`, with `signaller` in place
    /// of the kernel.
    fn run_tool(
        mode: Mode,
        source: &impl ProcessSource,
        arguments: &[&str],
        signaller: &FakeSignaller,
        stderr: &mut impl Write,
    ) -> Result<(String, ExitCode), Box<dyn std::error::Error>> {
        let Command::Run(options) = Options::parse(mode, arguments.iter().map(OsString::from))?
        else {
            return Err("expected options".into());
        };
        let invocation = Invocation {
            mode,
            program: OsStr::new(mode.name()),
            myself: Pid::from(1),
            charset: Charset::Utf8,
        };
        let selection = Selection::new(&options)?;
        let mut stdout = Vec::new();
        let status = run(
            &invocation,
            &selection,
            source,
            signaller,
            &mut stdout,
            stderr,
        );

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
        let selected: Vec<_> = processes
            .iter()
            .map(|process| Selected {
                task: process.linux_task(),
                output: Some(process.linux_task()),
            })
            .collect();
        let invocation = Invocation {
            mode: Mode::Pgrep,
            program: OsStr::new("pgrep"),
            myself: Pid::from(1),
            charset: Charset::Utf8,
        };
        let mut stdout = FailingOnce::default();

        report(&invocation, &options, &selected, &mut stdout);

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

    /// The standard output, standard error, exit status and signals of a run
    /// of `pkill`.
    type PkillRun = (String, String, ExitCode, Vec<(Pid, libc::c_int)>);

    /// Runs `pkill` with `arguments` and `signaller` against two processes
    /// called `sleep`, with pids 10 and 20.
    fn pkill(
        arguments: &[&str],
        signaller: &FakeSignaller,
    ) -> Result<PkillRun, Box<dyn std::error::Error>> {
        pkill_in(
            vec![process(10, "sleep"), process(20, "sleep")],
            arguments,
            signaller,
        )
    }

    /// Runs `pkill` with `arguments` and `signaller` against `processes`.
    fn pkill_in(
        processes: Vec<Process>,
        arguments: &[&str],
        signaller: &FakeSignaller,
    ) -> Result<PkillRun, Box<dyn std::error::Error>> {
        let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, processes);
        let mut stderr = Vec::new();
        let (stdout, status) = run_tool(Mode::Pkill, &source, arguments, signaller, &mut stderr)?;

        Ok((
            stdout,
            String::from_utf8(stderr)?,
            status,
            signaller.sent.take(),
        ))
    }

    #[rstest]
    #[case::default_signal(&["pkill", "sleep"], "", vec![(Pid::from(10), libc::SIGTERM), (Pid::from(20), libc::SIGTERM)])]
    #[case::named_signal(&["pkill", "-HUP", "sleep"], "", vec![(Pid::from(10), libc::SIGHUP), (Pid::from(20), libc::SIGHUP)])]
    #[case::echo(
        &["pkill", "-e", "-9", "sleep"],
        "sleep killed (pid 10)\nsleep killed (pid 20)\n",
        vec![(Pid::from(10), libc::SIGKILL), (Pid::from(20), libc::SIGKILL)]
    )]
    #[case::count(&["pkill", "-c", "sleep"], "2\n", vec![(Pid::from(10), libc::SIGTERM), (Pid::from(20), libc::SIGTERM)])]
    #[case::check_only(&["pkill", "-0", "sleep"], "", vec![(Pid::from(10), 0), (Pid::from(20), 0)])]
    fn pkill_signals_the_selected_processes(
        #[case] arguments: &[&str],
        #[case] stdout: &str,
        #[case] sent: Vec<(Pid, libc::c_int)>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            pkill(arguments, &FakeSignaller::default())?,
            (stdout.to_owned(), String::new(), ExitCode::SUCCESS, sent)
        );

        Ok(())
    }

    // procps-ng's `-e` prints its `cmdoutput` buffer. Without `--list-name`,
    // `--list-full` or `-Q`, it refreshes the buffer only for a task that
    // passes the filters when the command line has a pattern. The buffer starts
    // empty, as `Selected::output` explains.
    #[rstest]
    #[case::no_pattern(
        &["pkill", "-0", "-e", "-p", "10,20"],
        " killed (pid 10)\n killed (pid 20)\n",
        &[10, 20]
    )]
    #[case::inverse(
        &["pkill", "-0", "-e", "--inverse", "-p", "10", "^sleep$"],
        "sleep killed (pid 20)\nsleep killed (pid 30)\n",
        &[20, 30]
    )]
    #[case::count_after_echo(
        &["pkill", "-0", "-e", "-c", "sleep"],
        "sleep killed (pid 10)\nsleep killed (pid 30)\n2\n",
        &[10, 30]
    )]
    #[case::list_full_with_count(
        &["pkill", "-0", "-e", "-c", "--list-full", "sleep"],
        "[sleep] killed (pid 10)\n[sleep] killed (pid 30)\n2\n",
        &[10, 30]
    )]
    #[case::list_name(
        &["pkill", "-0", "-e", "--list-name", "-p", "20"],
        "cat killed (pid 20)\n",
        &[20]
    )]
    #[case::shell_quote(&["pkill", "-0", "-e", "-Q", "-p", "20"], "cat killed (pid 20)\n", &[20])]
    #[case::inverse_failing_the_pattern(
        &["pkill", "-0", "-e", "--inverse", "^sleep$"],
        "cat killed (pid 20)\n",
        &[20]
    )]
    fn pkill_echoes_procps_ng_s_output_buffer(
        #[case] arguments: &[&str],
        #[case] stdout: &str,
        #[case] signalled: &[libc::pid_t],
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            pkill_in(
                vec![
                    process(10, "sleep"),
                    process(20, "cat"),
                    process(30, "sleep"),
                ],
                arguments,
                &FakeSignaller::default(),
            )?,
            (
                stdout.to_owned(),
                String::new(),
                ExitCode::SUCCESS,
                signalled.iter().map(|&pid| (Pid::from(pid), 0)).collect()
            )
        );

        Ok(())
    }

    #[test]
    fn a_process_that_has_gone_is_not_echoed() -> Result<(), Box<dyn std::error::Error>> {
        let signaller = FakeSignaller {
            failures: vec![(Pid::from(10), Failure::Exited)],
            ..FakeSignaller::default()
        };

        assert_eq!(
            pkill(&["pkill", "-0", "-e", "sleep"], &signaller)?,
            (
                "sleep killed (pid 20)\n".to_owned(),
                String::new(),
                ExitCode::SUCCESS,
                vec![(Pid::from(20), 0)]
            )
        );

        Ok(())
    }

    #[rstest]
    #[case::denied(Failure::Denied, "Operation not permitted")]
    #[case::glibc_description(Failure::Os(libc::ENXIO), "No such device or address")]
    fn a_failed_signal_is_reported(
        #[case] failure: Failure,
        #[case] description: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let signaller = FakeSignaller {
            failures: vec![(Pid::from(10), failure)],
            ..FakeSignaller::default()
        };

        assert_eq!(
            pkill(&["pkill", "-e", "sleep"], &signaller)?,
            (
                "sleep killed (pid 20)\n".to_owned(),
                format!("pkill: killing pid 10 failed: {description}\n"),
                ExitCode::SUCCESS,
                vec![(Pid::from(20), libc::SIGTERM)]
            )
        );

        Ok(())
    }

    #[test]
    fn lightweight_signals_each_thread_s_process() -> Result<(), Box<dyn std::error::Error>> {
        let mut threaded = process(10, "sleep");
        threaded.threads = Some(Field::Available(vec![
            thread(900, b"main", RunState::Waiting),
            thread(901, b"worker", RunState::Waiting),
        ]));

        assert_eq!(
            pkill_in(
                vec![threaded],
                &["pkill", "--lightweight", "-e", "."],
                &FakeSignaller::default()
            )?,
            (
                "sleep killed (pid 900)\nworker killed (pid 901)\n".to_owned(),
                String::new(),
                ExitCode::SUCCESS,
                vec![
                    (Pid::from(10), libc::SIGTERM),
                    (Pid::from(10), libc::SIGTERM)
                ]
            )
        );

        Ok(())
    }

    #[test]
    fn pkill_warns_about_a_long_pattern() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            pkill(
                &["pkill", "sleep-for-a-long-time"],
                &FakeSignaller::default()
            )?,
            (
                String::new(),
                "pkill: pattern that searches for process name longer than 15 characters will \
                 result in zero matches\nTry `pkill -f' option to match against the complete \
                 command line.\n"
                    .to_owned(),
                ExitCode::from(1),
                Vec::new()
            )
        );

        Ok(())
    }

    #[rstest]
    #[case::exited(
        &["pkill", "sleep"],
        vec![(Pid::from(10), Failure::Exited), (Pid::from(20), Failure::Exited)],
        "",
        Vec::new()
    )]
    #[case::denied(
        &["pkill", "sleep"],
        vec![(Pid::from(10), Failure::Denied), (Pid::from(20), Failure::Denied)],
        "",
        vec!["pkill: killing pid 10 failed: Operation not permitted", "pkill: killing pid 20 failed: Operation not permitted"]
    )]
    #[case::count_of_selected_processes(
        &["pkill", "-c", "sleep"],
        vec![(Pid::from(10), Failure::Denied), (Pid::from(20), Failure::Denied)],
        "2\n",
        vec!["pkill: killing pid 10 failed: Operation not permitted", "pkill: killing pid 20 failed: Operation not permitted"]
    )]
    fn pkill_exits_with_status_1_when_it_signals_nothing(
        #[case] arguments: &[&str],
        #[case] failures: Vec<(Pid, Failure)>,
        #[case] stdout: &str,
        #[case] messages: Vec<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let signaller = FakeSignaller {
            failures,
            ..FakeSignaller::default()
        };
        let stderr = messages.iter().fold(String::new(), |mut stderr, message| {
            stderr.push_str(message);
            stderr.push('\n');
            stderr
        });

        assert_eq!(
            pkill(arguments, &signaller)?,
            (stdout.to_owned(), stderr, ExitCode::from(1), Vec::new())
        );

        Ok(())
    }

    #[rstest]
    #[case::exited(Reread::Failed(Failure::Exited), "")]
    #[case::pid_reused(Reread::Started(SystemTime::UNIX_EPOCH + Duration::from_secs(1)), "")]
    #[case::pid_reused_earlier(Reread::Started(SystemTime::UNIX_EPOCH - Duration::from_secs(1)), "")]
    #[case::unreadable(
        Reread::Failed(Failure::Os(libc::EIO)),
        "pkill: killing pid 10 failed: Input/output error\n"
    )]
    #[case::denied(
        Reread::Failed(Failure::Denied),
        "pkill: killing pid 10 failed: Operation not permitted\n"
    )]
    #[case::unsupported(
        Reread::Failed(Failure::Unsupported),
        "pkill: killing pid 10 failed: sysctl kern.proc does not provide data for process 10\n"
    )]
    fn pkill_rereads_the_start_time_before_it_signals(
        #[case] reread: Reread,
        #[case] stderr: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let signaller = FakeSignaller {
            rereads: vec![(Pid::from(10), reread)],
            ..FakeSignaller::default()
        };

        assert_eq!(
            pkill(&["pkill", "sleep"], &signaller)?,
            (
                String::new(),
                stderr.to_owned(),
                ExitCode::SUCCESS,
                vec![(Pid::from(20), libc::SIGTERM)]
            )
        );

        Ok(())
    }
}
