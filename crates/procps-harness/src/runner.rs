// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashSet,
    io::{self, Read, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::Duration,
};

use rustix::{
    io::Errno,
    process::{Pid, Signal, WaitId, WaitIdOptions},
};
use tempfile::TempDir;

use crate::{
    Credentials, Error, FixtureEnd, Outcome, Scenario, ScenarioFile,
    fixture::Fixtures,
    pids::skip_low_pids,
    programs::{Programs, new_directory},
    scenario::toml_files,
    session::{LEADER_ARGUMENT, LEADER_COMM, Report, Request},
    signal,
    spawn::SpawnAlone,
};

/// The procps-ng release that the golden files come from.
const REFERENCE_VERSION: &str = "4.0.7";

/// What the thread that waits for the session leader, or an [`Expiry`], sends
/// to the runner.
#[derive(Debug)]
enum Waited {
    /// The leader exited or stopped. The value is `waitid`'s result: the signal
    /// that stopped the leader, if one did, or the error.
    Changed(rustix::io::Result<Option<i32>>),
    /// The runner's expiry ended the wait.
    Expired,
}

/// Ends the wait of a run before its timeout passes. A test calls
/// [`Expiry::expire`] once the run's command has done what the test needs, so
/// the test need not wait for the timeout. The runner then kills the session
/// leader and its process group, as it does when the timeout passes, and the
/// run fails with [`Error::Expired`]. Clones refer to the same expiry.
#[derive(Clone, Debug, Default)]
pub struct Expiry(Arc<Mutex<ExpiryState>>);

/// The state of an [`Expiry`].
#[derive(Debug, Default)]
struct ExpiryState {
    expired: bool,
    /// The runs that are waiting, each with the number of its [`Watch`].
    waiting: Vec<(u64, mpsc::Sender<Waited>)>,
    /// The number of the next [`Watch`].
    next: u64,
}

/// A run's registration with an [`Expiry`]. Dropping it removes the run's
/// sender from the expiry.
struct Watch<'a> {
    expiry: &'a Expiry,
    number: u64,
}

impl Drop for Watch<'_> {
    fn drop(&mut self) {
        let mut state = self.expiry.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.waiting.retain(|(number, _)| *number != self.number);
    }
}

impl Expiry {
    /// Ends the wait of each run that is waiting on this expiry, and of each
    /// run that starts waiting later.
    ///
    /// ```
    /// use procps_harness::Expiry;
    ///
    /// Expiry::default().expire();
    /// ```
    pub fn expire(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.expired = true;

        for (_, waiting) in state.waiting.drain(..) {
            let _ = waiting.send(Waited::Expired);
        }
    }

    /// Sends [`Waited::Expired`] to `waiting` now if this expiry has expired,
    /// or makes `expire` send it later, while the returned [`Watch`] exists.
    fn watch(&self, waiting: mpsc::Sender<Waited>) -> Watch<'_> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let number = state.next;
        state.next += 1;

        if state.expired {
            let _ = waiting.send(Waited::Expired);
        } else {
            state.waiting.push((number, waiting));
        }

        Watch {
            expiry: self,
            number,
        }
    }
}

/// How a session leader ended. In each case the runner has sent `SIGKILL` to
/// the leader and to its process group.
enum SessionEnd {
    /// The leader exited before the timeout.
    Finished {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// A signal stopped the leader. The string is the signal's name.
    Stopped(String),
    /// The timeout passed.
    TimedOut,
    /// The runner's expiry ended the wait.
    Expired,
}

/// Reads everything from `stream`.
fn read_all(stream: Option<impl Read>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();

    if let Some(mut stream) = stream {
        stream.read_to_end(&mut bytes)?;
    }

    Ok(bytes)
}

/// A thread from [`read_in_background`].
type Reader = JoinHandle<std::io::Result<Vec<u8>>>;

/// Waits for a thread from [`read_in_background`], and returns what it read.
fn join(reader: Reader) -> Result<Vec<u8>, Error> {
    reader
        .join()
        .map_err(|_| Error::ReaderPanicked)?
        .map_err(Error::ReadLeader)
}

/// How long [`kill_group`] goes on killing a process group whose members are
/// still alive.
#[cfg(target_os = "macos")]
const GROUP_KILL_LIMIT: Duration = Duration::from_secs(10);

/// Sends `SIGKILL` to every process in the process group `group`.
///
/// XNU's `killpg` signals the members of the group that it finds when the call
/// starts, so a child that a member forks during the call survives it. The kill
/// is therefore repeated until `killpg` fails or [`GROUP_KILL_LIMIT`] passes.
/// `killpg` fails with `ESRCH` when the group does not exist, for example
/// before the leader calls `setsid`. It fails with `EPERM` once only zombies,
/// such as the unreaped leader, remain, and also when the remaining members
/// belong to a user that the runner may not signal. Those processes keep
/// running.
#[cfg(target_os = "macos")]
fn kill_group(group: Pid) -> Result<(), Error> {
    let deadline = std::time::Instant::now() + GROUP_KILL_LIMIT;

    loop {
        match rustix::process::kill_process_group(group, Signal::KILL) {
            Ok(()) => {}
            Err(Errno::SRCH | Errno::PERM) => return Ok(()),
            Err(errno) => return Err(Error::KillGroup(errno.into())),
        }

        if std::time::Instant::now() >= deadline {
            return Err(Error::GroupSurvived {
                group: group.as_raw_nonzero().get(),
                after: GROUP_KILL_LIMIT,
            });
        }

        std::thread::yield_now();
    }
}

/// Sends `SIGKILL` to every process in the process group `group`.
///
/// Linux delivers a group signal that arrives during a `fork` to the new child
/// as well, or abandons the fork when the signal is fatal, so one kill reaches
/// every member. Linux also accepts a kill of a group whose only member is the
/// unreaped leader, so repeating the kill until it fails would never end.
#[cfg(not(target_os = "macos"))]
fn kill_group(group: Pid) -> Result<(), Error> {
    match rustix::process::kill_process_group(group, Signal::KILL) {
        Ok(()) | Err(Errno::SRCH | Errno::PERM) => Ok(()),
        Err(errno) => Err(Error::KillGroup(errno.into())),
    }
}

/// Reads everything from `stream` on another thread.
fn read_in_background(stream: Option<impl Read + Send + 'static>) -> Result<Reader, Error> {
    std::thread::Builder::new()
        .spawn(move || read_all(stream))
        .map_err(Error::WatchThread)
}

/// A thread from [`write_in_background`].
type Writer = JoinHandle<std::io::Result<()>>;

/// Writes `bytes` to `stream` on another thread, then closes `stream`.
fn write_in_background(
    stream: Option<impl Write + Send + 'static>,
    bytes: Vec<u8>,
) -> Result<Writer, Error> {
    std::thread::Builder::new()
        .spawn(move || stream.map_or(Ok(()), |mut stream| stream.write_all(&bytes)))
        .map_err(Error::WatchThread)
}

/// The most attempts at one run. A run starts again when one of its processes
/// gets a pid outside the masked range.
const ATTEMPTS: usize = 20;

/// Runs scenarios with the tools in one directory.
///
/// Each run starts a session leader in a new session. The leader starts the
/// scenario's fixtures and then its command, so all of them are in that
/// session. The runner also starts a decoy for each fixture, with the same
/// process name, outside the session. A command that signals a decoy makes
/// the run fail, and [`Runner::generate`] rejects a scenario whose outcome
/// changes when decoys run. Runs on several threads or in several processes
/// therefore do not see each other's processes.
///
/// When the leader exits or stops, the timeout passes, or the runner's
/// [`Expiry`] ends the wait, the runner sends `SIGKILL` to the leader and to
/// its process group. A command must not stop the leader. The runner reports a
/// stop that it sees, but it can miss a stop that a `SIGCONT` undoes at once,
/// and the run then goes on, so the outcome depends on timing.
#[derive(Clone, Debug)]
pub struct Runner {
    tools: PathBuf,
    programs: Arc<Programs>,
    credentials: Option<Credentials>,
    timeout: Duration,
    expiry: Option<Expiry>,
}

/// The golden files that [`Runner::generate`] wrote and removed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Generation {
    /// The golden files that were written.
    pub written: Vec<PathBuf>,
    /// The golden files that no scenario had, which were removed.
    pub removed: Vec<PathBuf>,
}

impl Runner {
    /// A runner for the tools in `tools`. The session leader and fixtures run
    /// from copies of `host`, a binary whose `main` calls [`crate::host`]. The
    /// runner and its clones make one copy for each process name, and their
    /// runs share the copies.
    ///
    /// ```
    /// use procps_harness::Runner;
    ///
    /// let runner = Runner::new("/usr/bin", "target/debug/procps-harness");
    /// # let _ = runner;
    /// ```
    #[must_use]
    pub fn new(tools: impl Into<PathBuf>, host: impl Into<PathBuf>) -> Self {
        Self {
            tools: tools.into(),
            programs: Arc::new(Programs::new(host.into())),
            credentials: None,
            timeout: Duration::from_secs(60),
            expiry: None,
        }
    }

    /// The runner, changed to run each scenario's processes as another user
    /// and group. The runner must run as root.
    ///
    /// ```
    /// use procps_harness::{Credentials, Runner};
    ///
    /// let runner = Runner::new("/usr/bin", "target/debug/procps-harness").with_user(Credentials {
    ///     uid: 1000,
    ///     gid: 100,
    /// });
    /// # let _ = runner;
    /// ```
    #[must_use]
    pub fn with_user(self, credentials: Credentials) -> Self {
        Self {
            credentials: Some(credentials),
            ..self
        }
    }

    /// The runner, changed to kill the session leader and its process group
    /// if the leader has neither exited nor stopped after `timeout`. The
    /// default is a minute, which leaves time for the leader, the fixtures and
    /// the command to start and run on a busy machine.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use procps_harness::Runner;
    ///
    /// let runner =
    ///     Runner::new("/usr/bin", "target/debug/procps-harness").with_timeout(Duration::from_secs(1));
    /// # let _ = runner;
    /// ```
    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// The runner, changed so that `expiry` can end the wait for a session
    /// leader before the timeout passes.
    ///
    /// ```
    /// use procps_harness::{Expiry, Runner};
    ///
    /// let runner =
    ///     Runner::new("/usr/bin", "target/debug/procps-harness").with_expiry(Expiry::default());
    /// # let _ = runner;
    /// ```
    #[must_use]
    pub fn with_expiry(self, expiry: Expiry) -> Self {
        Self {
            expiry: Some(expiry),
            ..self
        }
    }

    /// Checks that the tools directory has the procps-ng release that the
    /// golden files come from.
    ///
    /// ```no_run
    /// use procps_harness::Runner;
    ///
    /// let runner = Runner::new("/nix/store/...-procps-4.0.7/bin", "procps-harness");
    /// runner.require_reference()?;
    /// # Ok::<(), procps_harness::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if `pgrep -V` cannot be run or reports another
    /// release.
    pub fn require_reference(&self) -> Result<(), Error> {
        let pgrep = self.tools.join("pgrep");
        let output = Command::new(&pgrep)
            .arg("-V")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn_alone()
            .and_then(Child::wait_with_output)
            .map_err(|source| Error::Io {
                path: pgrep,
                source,
            })?;
        let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();

        if found.ends_with(&format!("procps-ng {REFERENCE_VERSION}")) {
            return Ok(());
        }

        Err(Error::WrongReference {
            found,
            expected: REFERENCE_VERSION,
        })
    }

    /// Runs each scenario and writes its outcome to the scenario's golden file
    /// under `golden`, then removes the golden files that no scenario has.
    ///
    /// Each scenario runs twice, without and with decoys, and the two
    /// outcomes must be equal.
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// use procps_harness::{Runner, ScenarioFile};
    ///
    /// let runner = Runner::new("/nix/store/...-procps-4.0.7/bin", "procps-harness");
    /// let scenarios = ScenarioFile::discover(Path::new("scenarios"))?;
    ///
    /// let generation = runner.generate(&scenarios, Path::new("golden"))?;
    /// # let _ = generation;
    /// # Ok::<(), procps_harness::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if a scenario cannot be run, its outcome changes when
    /// decoys run, or a golden file cannot be written or removed.
    pub fn generate(&self, scenarios: &[ScenarioFile], golden: &Path) -> Result<Generation, Error> {
        let mut generation = Generation::default();

        for file in scenarios {
            let alone = self.run_with(&file.scenario, false)?;
            let with_decoys = self.run_with(&file.scenario, true)?;

            if alone != with_decoys {
                return Err(Error::Unconfined {
                    tool: file.tool.clone(),
                    name: file.name.clone(),
                    alone: Box::new(alone),
                    with_decoys: Box::new(with_decoys),
                });
            }

            let path = file.golden_path(golden);
            alone.write(&path)?;
            generation.written.push(path);
        }

        if !golden.exists() {
            return Ok(generation);
        }

        let written: HashSet<_> = generation.written.iter().cloned().collect();

        for stale in toml_files(golden)? {
            if written.contains(&stale.path) {
                continue;
            }

            std::fs::remove_file(&stale.path).map_err(|source| Error::Io {
                path: stale.path.clone(),
                source,
            })?;
            generation.removed.push(stale.path);
        }

        Ok(generation)
    }

    /// Runs the scenario, with decoys, and returns its outcome with known pids
    /// masked.
    ///
    /// The command's environment contains only `PATH` (the tools directory),
    /// `HOME`, `TZ=UTC`, `LC_ALL=C` and the variables from the scenario's
    /// `[environment]` table. The table can replace `TZ`, and the leader leaves
    /// out `LC_ALL=C` if the table sets a locale variable.
    ///
    /// ```no_run
    /// use procps_harness::{Runner, Scenario};
    ///
    /// let runner = Runner::new("target/debug", "target/debug/procps-harness");
    /// let scenario: Scenario =
    ///     "description = \"pgrep -V\"\ncommand = [\"pgrep\", \"-V\"]\n".parse()?;
    ///
    /// let outcome = runner.run(&scenario)?;
    /// # let _ = outcome;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if a process cannot be started, the command does not
    /// finish in time, the runner's expiry ends the wait, the command signals a
    /// decoy, a signal terminates or stops the session leader, members of the
    /// leader's process group survive the runner's kills, or the runner cannot
    /// watch, write to, read from or kill the session leader.
    pub fn run(&self, scenario: &Scenario) -> Result<Outcome, Error> {
        self.run_with(scenario, true)
    }

    fn run_with(&self, scenario: &Scenario, decoys: bool) -> Result<Outcome, Error> {
        let programs = self.programs.prepare(
            std::iter::once(LEADER_COMM).chain(
                scenario
                    .fixtures
                    .iter()
                    .map(|fixture| fixture.comm.as_str()),
            ),
        )?;
        let run_directory = self.run_directory()?;
        let decoys = decoys
            .then(|| Fixtures::start(&scenario.fixtures, &programs, self.credentials))
            .transpose()?;

        let request = Request {
            tools: self.tools.clone(),
            programs: programs.clone(),
            home: run_directory.path().join("home"),
            scenario: scenario.clone(),
        };
        let outcome = self.lead(&request, &programs)?;

        let Some(decoys) = decoys else {
            return Ok(outcome);
        };

        for (name, end) in decoys.finish()? {
            if end == FixtureEnd::Running {
                continue;
            }

            let comm = scenario
                .fixtures
                .iter()
                .find(|fixture| fixture.name == name)
                .map_or(name, |fixture| fixture.comm.clone());

            return Err(Error::DecoyDisturbed { comm, end });
        }

        Ok(outcome)
    }

    /// Starts session leaders until one gets suitable pids for its processes,
    /// and returns that leader's outcome.
    fn lead(&self, request: &Request, programs: &Path) -> Result<Outcome, Error> {
        let text = toml::to_string(request)?.into_bytes();
        let leader = programs.join(LEADER_COMM);
        let mut last = 0;

        for _ in 0..ATTEMPTS {
            let mut command = Command::new(&leader);
            command
                .arg0(LEADER_COMM)
                .arg(LEADER_ARGUMENT)
                .env_clear()
                .current_dir(&request.home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            if let Some(credentials) = self.credentials {
                credentials.apply(&mut command);
            }

            let io_error = |source| Error::Io {
                path: leader.clone(),
                source,
            };
            let child = command.spawn_alone().map_err(io_error)?;

            let (status, stdout, stderr) = match self.wait_for_session(child, &text)? {
                SessionEnd::Finished {
                    status,
                    stdout,
                    stderr,
                } => (status, stdout, stderr),
                SessionEnd::Stopped(signal) => return Err(Error::LeaderStopped(signal)),
                SessionEnd::TimedOut => {
                    return Err(Error::Timeout {
                        after: self.timeout,
                    });
                }
                SessionEnd::Expired => return Err(Error::Expired),
            };

            if let Some(signal) = status.signal() {
                return Err(Error::LeaderSignalled(signal::name(signal)));
            }

            if !status.success() {
                return Err(Error::Session {
                    message: format!("{status}: {}", String::from_utf8_lossy(&stderr).trim()),
                });
            }

            let report = toml::from_str(&String::from_utf8_lossy(&stdout))
                .map_err(|source| Error::Report { source })?;

            match report {
                Report::Finished { outcome } => return Ok(outcome),
                Report::PidOutOfRange { pid } => {
                    last = pid;
                    skip_low_pids()?;
                }
                Report::Failed { failure } => return Err(failure.into()),
            }
        }

        Err(Error::NoFreePids { last })
    }

    /// Sends `request` to the session leader, waits until the leader exits or
    /// stops, the timeout passes or the expiry ends the wait, then sends
    /// `SIGKILL` to the leader and to its process group.
    fn wait_for_session(&self, mut leader: Child, request: &[u8]) -> Result<SessionEnd, Error> {
        let pid = Pid::from_child(&leader);
        let (sender, receiver) = mpsc::channel();
        let _watch = self
            .expiry
            .as_ref()
            .map(|expiry| expiry.watch(sender.clone()));

        let start = || -> Result<(Reader, Reader, Writer), Error> {
            let stdout = read_in_background(leader.stdout.take())?;
            let stderr = read_in_background(leader.stderr.take())?;
            let stdin = write_in_background(leader.stdin.take(), request.to_vec())?;

            // `NOWAIT` leaves the leader unreaped, so its pid cannot belong to
            // another process until `leader.wait` below. Darwin reports a stop
            // even without `STOPPED`, but Linux reports one only with it.
            std::thread::Builder::new()
                .spawn(move || {
                    let changed = loop {
                        match rustix::process::waitid(
                            WaitId::Pid(pid),
                            WaitIdOptions::EXITED | WaitIdOptions::STOPPED | WaitIdOptions::NOWAIT,
                        ) {
                            Err(Errno::INTR) => {}
                            changed => break changed,
                        }
                    };
                    let _ =
                        sender
                            .send(Waited::Changed(changed.map(|change| {
                                change.and_then(|change| change.stopping_signal())
                            })));
                })
                .map_err(Error::WatchThread)?;

            Ok((stdout, stderr, stdin))
        };
        let threads = start();
        let waited = threads
            .as_ref()
            .ok()
            .map(|_| receiver.recv_timeout(self.timeout));

        // The leader calls `setsid` itself, so the timeout can pass before its
        // process group exists. The leader is killed first, so that it cannot
        // create the group after the group is killed. If the kill fails,
        // return without reaping the leader, because `leader.wait` would block
        // until the leader exits.
        let leader_killed = rustix::process::kill_process(pid, Signal::KILL);
        let group_killed = kill_group(pid);
        leader_killed.map_err(|errno| Error::KillLeader(errno.into()))?;

        // The waiting thread returns once the leader has died. Waiting for it
        // here keeps it from calling `waitid` after the leader is reaped, when
        // the pid can belong to another child of the runner.
        if matches!(
            waited,
            Some(Err(RecvTimeoutError::Timeout) | Ok(Waited::Expired))
        ) {
            while let Ok(Waited::Expired) = receiver.recv() {}
        }

        let status = leader.wait();
        group_killed?;
        let status = status.map_err(Error::WaitLeader)?;
        let (stdout, stderr, stdin) = threads?;
        let stdout = join(stdout)?;
        let stderr = join(stderr)?;

        match stdin.join().map_err(|_| Error::WriterPanicked)? {
            // The leader can exit, or the runner can kill it, before it has
            // read the whole request. The outcome then reports how the run
            // ended, so the broken pipe is not an error.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
            written => written.map_err(Error::WriteRequest)?,
        }

        let stopped = match waited {
            Some(Ok(Waited::Changed(changed))) => {
                changed.map_err(|errno| Error::WaitLeader(errno.into()))?
            }
            Some(Ok(Waited::Expired)) => return Ok(SessionEnd::Expired),
            Some(Err(RecvTimeoutError::Timeout)) => return Ok(SessionEnd::TimedOut),
            Some(Err(RecvTimeoutError::Disconnected)) | None => return Err(Error::WaiterLost),
        };

        if let Some(signal) = stopped {
            return Ok(SessionEnd::Stopped(signal::name(signal)));
        }

        Ok(SessionEnd::Finished {
            status,
            stdout,
            stderr,
        })
    }

    /// Creates a directory for one run, which contains an empty home directory
    /// for the command.
    fn run_directory(&self) -> Result<TempDir, Error> {
        let directory = new_directory()?;
        let home = directory.path().join("home");
        let io_error = |source| Error::Io {
            path: home.clone(),
            source,
        };

        std::fs::create_dir(&home).map_err(io_error)?;

        if let Some(credentials) = self.credentials {
            std::os::unix::fs::chown(&home, Some(credentials.uid), Some(credentials.gid))
                .map_err(io_error)?;
        }

        Ok(directory)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{PoisonError, mpsc};

    use pretty_assertions::assert_eq;

    use super::Expiry;

    #[test]
    fn a_run_stops_waiting_on_the_expiry_when_its_wait_ends() {
        let expiry = Expiry::default();
        let waiting = || {
            expiry
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .waiting
                .len()
        };
        let (sender, _receiver) = mpsc::channel();

        let watch = expiry.watch(sender);
        let during = waiting();
        drop(watch);

        assert_eq!((during, waiting()), (1, 0));
    }
}
