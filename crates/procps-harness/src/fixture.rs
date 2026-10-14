// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nix::sys::signal::{SigSet, SigmaskHow};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    process::{Pid, Signal, WaitOptions},
};

use crate::{Credentials, Error, FixtureEnd, scenario::FixtureSpec, signal, spawn::SpawnAlone};

/// The argument that makes a host binary run as a fixture process.
pub(crate) const FIXTURE_ARGUMENT: &str = "--procps-harness-fixture";

const READY: &str = "ready\n";

/// How long a fixture has to report that it is ready: as long as a run's
/// default timeout, so that a fixture on a heavily loaded machine has time to
/// start.
const READY_LIMIT: Duration = Duration::from_secs(60);

/// The options that a fixture process receives after [`FIXTURE_ARGUMENT`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct FixtureOptions {
    /// The number of threads to start in addition to the main thread.
    threads: u8,
    /// The signals to catch.
    catch: Vec<Signal>,
}

impl FixtureOptions {
    const THREADS: &str = "--threads=";
    const CATCH: &str = "--catch=";

    /// The options for the process that `spec` describes. Parsing the scenario
    /// has already rejected the signals that cannot be caught.
    fn for_spec(spec: &FixtureSpec) -> Self {
        Self {
            threads: spec.threads,
            catch: spec
                .catch
                .iter()
                .filter_map(|name| signal::catchable(name))
                .collect(),
        }
    }

    /// The arguments that encode these options. Golden files record the
    /// fixtures' command lines. An option on every fixture would change each
    /// golden file that shows a fixture's command line, so a fixture receives
    /// only the options that differ from the default.
    fn arguments(&self) -> Vec<String> {
        let threads = (self.threads > 0).then(|| format!("{}{}", Self::THREADS, self.threads));
        let catch = self
            .catch
            .iter()
            .map(|signal| format!("{}{}", Self::CATCH, signal::name(signal.as_raw())));

        threads.into_iter().chain(catch).collect()
    }

    /// Reads the options from `arguments`, or returns `None` if one of them is
    /// not an option that [`FixtureOptions::arguments`] writes.
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Option<Self> {
        let mut options = Self::default();

        for argument in arguments {
            let argument = argument.into_string().ok()?;

            if let Some(count) = argument.strip_prefix(Self::THREADS) {
                options.threads = count.parse().ok()?;
                continue;
            }

            let name = argument.strip_prefix(Self::CATCH)?;
            options.catch.push(signal::catchable(name)?);
        }

        Some(options)
    }
}

/// Runs as a fixture process: catches the signals and starts the threads that
/// its options specify, then reports that it is ready and waits until its
/// standard input closes. It then writes a [`FixtureEnd`] to standard output:
/// the signals that its handlers caught, or `running`.
pub(crate) fn fixture_main() -> ExitCode {
    let Some(options) = FixtureOptions::parse(std::env::args_os().skip(2)) else {
        return ExitCode::FAILURE;
    };
    let wait = || -> io::Result<()> {
        let caught = options
            .catch
            .iter()
            .map(|&signal| {
                let flag = Arc::new(AtomicBool::new(false));
                signal_hook::flag::register(signal.as_raw(), Arc::clone(&flag))?;
                Ok((signal, flag))
            })
            .collect::<io::Result<Vec<_>>>()?;

        // A signal sent to the process goes to a thread that does not block
        // it, and the handler runs on that thread. The other threads inherit
        // this mask and block every signal, so each handler runs on the main
        // thread before its read of standard input returns, and the flags are
        // complete when the main thread reads them.
        let unblocked = SigSet::all().thread_swap_mask(SigmaskHow::SIG_BLOCK)?;

        for _ in 0..options.threads {
            thread::Builder::new().spawn(|| {
                loop {
                    thread::park();
                }
            })?;
        }

        unblocked.thread_set_mask()?;

        let mut stdout = io::stdout().lock();
        stdout.write_all(READY.as_bytes())?;
        stdout.flush()?;

        io::copy(&mut io::stdin().lock(), &mut io::sink())?;

        let caught: Vec<String> = caught
            .iter()
            .filter(|(_, flag)| flag.load(Ordering::Relaxed))
            .map(|(signal, _)| signal::name(signal.as_raw()))
            .collect();
        let end = if caught.is_empty() {
            FixtureEnd::Running
        } else {
            FixtureEnd::Caught(caught)
        };

        writeln!(stdout, "{end}")?;
        stdout.flush()
    };

    match wait() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// What a fixture wrote before its readiness deadline.
#[derive(Debug, Eq, PartialEq)]
enum Reading {
    /// A line, or the text before the end of the output.
    Line(String),
    /// The text that had arrived when the deadline passed, without a newline.
    TimedOut(String),
}

/// Reads a line from `reader` until `deadline`. Once the deadline has passed,
/// the read returns what it has read, even if more output is available, so a
/// writer cannot make it run past the deadline.
fn read_line_by<R: Read + AsFd>(
    reader: &mut BufReader<R>,
    deadline: Instant,
) -> io::Result<Reading> {
    let mut line = Vec::new();

    loop {
        if reader.buffer().is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            let wait = Timespec::try_from(left).map_err(io::Error::other)?;
            let mut ready = [PollFd::new(reader.get_ref(), PollFlags::IN)];

            if poll(&mut ready, Some(&wait))? == 0 {
                return Ok(Reading::TimedOut(
                    String::from_utf8_lossy(&line).into_owned(),
                ));
            }
        }

        let available = reader.fill_buf()?;

        if available.is_empty() {
            return Ok(Reading::Line(String::from_utf8_lossy(&line).into_owned()));
        }

        if let Some(end) = available.iter().position(|&byte| byte == b'\n') {
            line.extend_from_slice(&available[..=end]);
            reader.consume(end + 1);
            return Ok(Reading::Line(String::from_utf8_lossy(&line).into_owned()));
        }

        let length = available.len();
        line.extend_from_slice(available);
        reader.consume(length);

        if Instant::now() >= deadline {
            return Ok(Reading::TimedOut(
                String::from_utf8_lossy(&line).into_owned(),
            ));
        }
    }
}

/// What a fixture was doing when the runner stopped waiting for it, as
/// `waitpid` reports it.
#[derive(Clone, Debug, Eq, PartialEq, derive_more::Display)]
pub enum FixtureState {
    /// The fixture was still running.
    #[display("was still running")]
    Running,
    /// A signal, such as `SIGSTOP`, had stopped the fixture.
    #[display("was stopped by {_0}")]
    Stopped(String),
    /// A signal, such as `SIGKILL`, had terminated the fixture.
    #[display("was killed by {_0}")]
    Killed(String),
    /// The fixture had exited with this status.
    #[display("had exited with status {_0}")]
    Exited(i32),
}

/// Running fixture processes. Dropping this kills the ones that are still
/// running.
pub(crate) struct Fixtures(Vec<Fixture>);

struct Fixture {
    name: String,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    reaped: bool,
}

impl Fixtures {
    /// Starts one process for each spec, from the program `<directory>/<comm>`.
    /// The system reports the program's file name as the process name.
    pub(crate) fn start(
        specs: &[FixtureSpec],
        directory: &Path,
        credentials: Option<Credentials>,
    ) -> Result<Self, Error> {
        let mut fixtures = Self(Vec::with_capacity(specs.len()));

        for spec in specs {
            fixtures
                .0
                .push(Fixture::start(spec, directory, credentials)?);
        }

        Ok(fixtures)
    }

    /// Each fixture's name and pid, in the order that they started.
    pub(crate) fn pids(&self) -> impl Iterator<Item = (String, u32)> {
        self.0
            .iter()
            .map(|fixture| (fixture.name.clone(), fixture.child.id()))
    }

    /// Closes each fixture's standard input and reports what had happened to
    /// it. A running fixture exits when its input closes. A fixture that a
    /// signal had already terminated or stopped reports that signal.
    pub(crate) fn finish(mut self) -> Result<BTreeMap<String, FixtureEnd>, Error> {
        self.0
            .iter_mut()
            .map(|fixture| Ok((fixture.name.clone(), fixture.finish()?)))
            .collect()
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        for fixture in &mut self.0 {
            fixture.kill();
        }
    }
}

impl Fixture {
    fn start(
        spec: &FixtureSpec,
        directory: &Path,
        credentials: Option<Credentials>,
    ) -> Result<Self, Error> {
        let failure = |source| Error::Fixture {
            name: spec.name.clone(),
            source,
        };
        let mut command = Command::new(directory.join(&spec.comm));
        command
            .arg0(&spec.comm)
            .arg(FIXTURE_ARGUMENT)
            .args(FixtureOptions::for_spec(spec).arguments())
            .env_clear()
            .envs(&spec.environment)
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(credentials) = credentials {
            credentials.apply(&mut command);
        }

        let mut child = command.spawn_alone().map_err(failure)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new);
        let mut fixture = Self {
            name: spec.name.clone(),
            child,
            stdin,
            stdout,
            reaped: false,
        };

        fixture.wait_until_ready(READY_LIMIT)?;

        Ok(fixture)
    }

    /// Waits at most `limit` for the fixture to report that it is ready. A
    /// fixture that is not ready by then is killed.
    fn wait_until_ready(&mut self, limit: Duration) -> Result<(), Error> {
        let name = self.name.clone();
        let failure = |source| Error::Fixture {
            name: name.clone(),
            source,
        };
        let deadline = Instant::now() + limit;
        let reading = self
            .stdout
            .as_mut()
            .map(|stdout| read_line_by(stdout, deadline))
            .transpose()
            .map_err(failure)?
            .unwrap_or(Reading::Line(String::new()));

        let line = match reading {
            Reading::Line(line) => line,
            Reading::TimedOut(output) => {
                let state = self.state()?;
                self.kill();

                return Err(Error::FixtureNotReadyInTime {
                    name: self.name.clone(),
                    after: limit,
                    state,
                    output,
                });
            }
        };

        if line != READY {
            // The fixture closes its standard output only when it exits.
            drop(self.stdin.take());
            let status = self.child.wait().map_err(failure)?;
            self.reaped = true;

            return Err(Error::FixtureNotReady {
                name: self.name.clone(),
                output: line,
                status,
            });
        }

        Ok(())
    }

    fn finish(&mut self) -> Result<FixtureEnd, Error> {
        let failure = |source: rustix::io::Errno| Error::Fixture {
            name: self.name.clone(),
            source: source.into(),
        };
        let pid = Pid::from_child(&self.child);

        drop(self.stdin.take());

        // `Child::wait` blocks while the fixture is stopped. `waitpid` with
        // `WUNTRACED` returns and reports the stop.
        let Some((_, status)) =
            rustix::process::waitpid(Some(pid), WaitOptions::UNTRACED).map_err(failure)?
        else {
            return Err(failure(rustix::io::Errno::CHILD));
        };
        self.reaped = !status.stopped();

        if let Some(signal) = status.stopping_signal() {
            self.kill();
            return Ok(FixtureEnd::Stopped(signal::name(signal)));
        }

        if let Some(signal) = status.terminating_signal() {
            return Ok(FixtureEnd::Killed(signal::name(signal)));
        }

        if status.exit_status() == Some(0) {
            return self.read_end();
        }

        Err(Error::FixtureExited {
            name: self.name.clone(),
            status: status.as_raw(),
        })
    }

    /// Reads what the fixture wrote when its standard input closed: `running`,
    /// or the signals that its handlers caught.
    fn read_end(&mut self) -> Result<FixtureEnd, Error> {
        let mut text = String::new();

        if let Some(stdout) = &mut self.stdout {
            stdout
                .read_to_string(&mut text)
                .map_err(|source| Error::Fixture {
                    name: self.name.clone(),
                    source,
                })?;
        }

        match text.strip_suffix('\n').unwrap_or(&text).parse()? {
            end @ (FixtureEnd::Running | FixtureEnd::Caught(_)) => Ok(end),
            FixtureEnd::Killed(_) | FixtureEnd::Stopped(_) => Err(Error::UnknownFixtureEnd(text)),
        }
    }

    /// What the fixture is doing now. A fixture that has ended is reaped.
    fn state(&mut self) -> Result<FixtureState, Error> {
        let pid = Pid::from_child(&self.child);
        let status =
            rustix::process::waitpid(Some(pid), WaitOptions::UNTRACED | WaitOptions::NOHANG)
                .map_err(|source| Error::Fixture {
                    name: self.name.clone(),
                    source: source.into(),
                })?;
        let Some((_, status)) = status else {
            return Ok(FixtureState::Running);
        };

        if let Some(signal) = status.stopping_signal() {
            return Ok(FixtureState::Stopped(signal::name(signal)));
        }

        self.reaped = true;

        if let Some(signal) = status.terminating_signal() {
            return Ok(FixtureState::Killed(signal::name(signal)));
        }

        Ok(FixtureState::Exited(status.exit_status().unwrap_or(-1)))
    }

    /// Kills and reaps the fixture if it has not been reaped.
    fn kill(&mut self) {
        if self.reaped {
            return;
        }

        drop(self.stdin.take());

        let pid = Pid::from_child(&self.child);
        let _ = rustix::process::kill_process(pid, Signal::KILL);
        let _ = rustix::process::waitpid(Some(pid), WaitOptions::empty());

        self.reaped = true;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        io::{self, BufReader, Write},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};

    use super::{Fixture, FixtureOptions, FixtureState, Reading, read_line_by};
    use crate::Error;

    #[rstest]
    #[case::default(FixtureOptions::default(), &[])]
    #[case::threads(FixtureOptions { threads: 2, catch: vec![] }, &["--threads=2"])]
    #[case::catch(
        FixtureOptions { threads: 0, catch: vec![Signal::HUP, Signal::USR1] },
        &["--catch=SIGHUP", "--catch=SIGUSR1"],
    )]
    fn the_options_round_trip_through_their_arguments(
        #[case] options: FixtureOptions,
        #[case] expected: &[&str],
    ) {
        let arguments = options.arguments();

        assert_eq!(
            (
                arguments.clone(),
                FixtureOptions::parse(arguments.into_iter().map(OsString::from))
            ),
            (
                expected
                    .iter()
                    .map(|&argument| argument.to_owned())
                    .collect(),
                Some(options)
            )
        );
    }

    #[rstest]
    #[case::unknown("--other")]
    #[case::count("--threads=many")]
    #[case::uncatchable("--catch=SIGKILL")]
    fn an_invalid_argument_is_rejected(#[case] argument: &str) {
        assert_eq!(FixtureOptions::parse([OsString::from(argument)]), None);
    }

    // A deadline that has passed returns what has arrived. A later deadline is
    // never reached, because each case ends before it.
    #[rstest]
    #[case::line(b"ready\n", true, false, Reading::Line("ready\n".to_owned()))]
    #[case::line_after_the_deadline(b"ready\n", true, true, Reading::Line("ready\n".to_owned()))]
    #[case::closed(b"", false, false, Reading::Line(String::new()))]
    #[case::partial_then_closed(b"rea", false, false, Reading::Line("rea".to_owned()))]
    #[case::silent(b"", true, true, Reading::TimedOut(String::new()))]
    #[case::partial(b"rea", true, true, Reading::TimedOut("rea".to_owned()))]
    #[case::more_output_after_the_deadline(
        &[b'x'; 64],
        true,
        true,
        Reading::TimedOut("x".repeat(16))
    )]
    fn a_line_is_read_only_if_it_ends_in_time(
        #[case] written: &[u8],
        #[case] stays_open: bool,
        #[case] deadline_passed: bool,
        #[case] expected: Reading,
    ) -> io::Result<()> {
        let (reader, mut writer) = io::pipe()?;
        writer.write_all(written)?;
        let open = stays_open.then_some(writer);
        let deadline = if deadline_passed {
            Instant::now()
        } else {
            Instant::now() + Duration::from_secs(60)
        };

        assert_eq!(
            read_line_by(&mut BufReader::with_capacity(16, reader), deadline)?,
            expected
        );

        drop(open);
        Ok(())
    }

    /// The variable that makes [`waits_for_its_input`] run as a child.
    const CHILD: &str = "PROCPS_HARNESS_TEST_CHILD";

    /// The line that [`waits_for_its_input`] writes once it waits.
    const WAITING: &str = "waiting\n";

    /// Runs as a child of the tests below. It reports that it waits, then waits
    /// until its standard input closes.
    #[test]
    #[ignore = "the tests below run it as a child"]
    fn waits_for_its_input() -> io::Result<()> {
        if std::env::var_os(CHILD).is_none() {
            return Ok(());
        }

        let mut stdout = io::stdout().lock();
        stdout.write_all(WAITING.as_bytes())?;
        stdout.flush()?;

        io::copy(&mut io::stdin().lock(), &mut io::sink())?;
        Ok(())
    }

    /// Why a test could not start [`waits_for_its_input`] as a fixture.
    #[derive(Debug, thiserror::Error)]
    enum WaitingError {
        #[error("cannot start the child or read its output")]
        Io(#[from] io::Error),
        #[error("the child exited before it waited; its output was {0:?}")]
        Exited(String),
        #[error("the child did not wait within a minute; its output was {0:?}")]
        TimedOut(String),
    }

    /// A fixture that runs [`waits_for_its_input`], once the child has reported
    /// that it waits. libtest writes its own lines first.
    fn waiting_fixture() -> Result<Fixture, WaitingError> {
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--ignored",
                "--exact",
                "--quiet",
                "fixture::tests::waits_for_its_input",
            ])
            .env(CHILD, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut fixture = Fixture {
            name: "a1".to_owned(),
            stdin: child.stdin.take(),
            stdout: child.stdout.take().map(BufReader::new),
            child,
            reaped: false,
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut output = String::new();

        while let Some(stdout) = fixture.stdout.as_mut() {
            match read_line_by(stdout, deadline)? {
                Reading::Line(line) if line == WAITING => return Ok(fixture),
                Reading::Line(line) if line.is_empty() => {
                    return Err(WaitingError::Exited(output));
                }
                Reading::Line(line) => output.push_str(&line),
                Reading::TimedOut(line) => {
                    output.push_str(&line);
                    return Err(WaitingError::TimedOut(output));
                }
            }
        }

        Err(WaitingError::Exited(output))
    }

    /// Waits until the fixture's process has ended or stopped, without reaping
    /// it.
    fn wait_for_change(fixture: &Fixture, change: WaitIdOptions) -> io::Result<()> {
        let pid = Pid::from_child(&fixture.child);
        rustix::process::waitid(WaitId::Pid(pid), change | WaitIdOptions::NOWAIT)?;

        Ok(())
    }

    // Each wait also returns when the child exits, so a child that exits too
    // early fails the test and does not hang it.
    #[test]
    fn a_fixture_s_state_is_what_waitpid_reports() -> Result<(), Box<dyn std::error::Error>> {
        let mut running = waiting_fixture()?;
        let mut killed = waiting_fixture()?;
        let mut exited = waiting_fixture()?;
        let mut states = Vec::new();

        states.push((running.state()?, running.reaped));
        rustix::process::kill_process(Pid::from_child(&running.child), Signal::STOP)?;
        wait_for_change(&running, WaitIdOptions::STOPPED | WaitIdOptions::EXITED)?;
        states.push((running.state()?, running.reaped));

        rustix::process::kill_process(Pid::from_child(&killed.child), Signal::TERM)?;
        wait_for_change(&killed, WaitIdOptions::EXITED)?;
        states.push((killed.state()?, killed.reaped));

        drop(exited.stdin.take());
        wait_for_change(&exited, WaitIdOptions::EXITED)?;
        states.push((exited.state()?, exited.reaped));

        for fixture in [&mut running, &mut killed, &mut exited] {
            fixture.kill();
        }

        assert_eq!(
            states,
            [
                (FixtureState::Running, false),
                (FixtureState::Stopped("SIGSTOP".to_owned()), false),
                (FixtureState::Killed("SIGTERM".to_owned()), true),
                (FixtureState::Exited(0), true),
            ]
        );

        Ok(())
    }

    #[test]
    fn a_fixture_that_is_not_ready_in_time_is_killed() -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = waiting_fixture()?;

        let result = fixture.wait_until_ready(Duration::ZERO);

        assert_matches!(
            (result, fixture.reaped),
            (
                Err(Error::FixtureNotReadyInTime { name, after, state, output }),
                true
            ) if name == "a1"
                && after == Duration::ZERO
                && state == FixtureState::Running
                && output.is_empty()
        );

        Ok(())
    }
}
