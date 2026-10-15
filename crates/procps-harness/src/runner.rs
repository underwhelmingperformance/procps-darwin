// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashSet,
    io::{self, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
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
    scenario::toml_files,
    session::{LEADER_ARGUMENT, LEADER_COMM, Report, Request},
    signal,
    spawn::SpawnAlone,
};

/// The procps-ng release that the golden files come from.
const REFERENCE_VERSION: &str = "4.0.7";

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
/// When the leader exits or stops, or the timeout passes, the runner sends
/// `SIGKILL` to the leader and to its process group. A command must not stop
/// the leader. The runner reports a stop that it sees, but it can miss a stop
/// that a `SIGCONT` undoes at once, and the run then goes on, so the outcome
/// depends on timing.
#[derive(Clone, Debug)]
pub struct Runner {
    tools: PathBuf,
    host: PathBuf,
    credentials: Option<Credentials>,
    timeout: Duration,
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
    /// A runner for the tools in `tools`. The session leader and fixtures are
    /// copies of `host`, a binary whose `main` calls [`crate::host`].
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
            host: host.into(),
            credentials: None,
            timeout: Duration::from_secs(60),
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
    /// The command runs with an empty environment apart from `PATH`, which is
    /// the tools directory, `HOME`, `LC_ALL=C` and `TZ=UTC`.
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
    /// finish in time, the command signals a decoy, a signal terminates or
    /// stops the session leader, or the runner cannot watch, write to, read
    /// from or kill the session leader.
    pub fn run(&self, scenario: &Scenario) -> Result<Outcome, Error> {
        self.run_with(scenario, true)
    }

    fn run_with(&self, scenario: &Scenario, decoys: bool) -> Result<Outcome, Error> {
        let programs = self.programs(scenario)?;
        let home = programs.path().join("home");
        let decoys = decoys
            .then(|| Fixtures::start(&scenario.fixtures, programs.path(), self.credentials))
            .transpose()?;

        let request = Request {
            tools: self.tools.clone(),
            programs: programs.path().to_owned(),
            home,
            scenario: scenario.clone(),
        };
        let outcome = self.lead(&request, programs.path())?;

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
    /// stops, or the timeout passes, then sends `SIGKILL` to the leader and to
    /// its process group.
    fn wait_for_session(&self, mut leader: Child, request: &[u8]) -> Result<SessionEnd, Error> {
        let pid = Pid::from_child(&leader);
        let (sender, receiver) = mpsc::channel();
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
                    let _ = sender.send(
                        changed.map(|change| change.and_then(|change| change.stopping_signal())),
                    );
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
        if waited == Some(Err(RecvTimeoutError::Timeout)) {
            let _ = receiver.recv();
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
            Some(Ok(changed)) => changed.map_err(|errno| Error::WaitLeader(errno.into()))?,
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

    /// Creates a directory with the session leader's program, a program for
    /// each fixture's process name, and an empty home directory for the
    /// command.
    fn programs(&self, scenario: &Scenario) -> Result<TempDir, Error> {
        let directory = tempfile::Builder::new()
            .prefix("procps-harness-")
            .tempdir()
            .map_err(|source| Error::Io {
                path: std::env::temp_dir(),
                source,
            })?;
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io { path, source }
        };

        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(io_error(directory.path()))?;

        let names: HashSet<&str> = std::iter::once(LEADER_COMM)
            .chain(
                scenario
                    .fixtures
                    .iter()
                    .map(|fixture| fixture.comm.as_str()),
            )
            .collect();

        for name in names {
            let program = directory.path().join(name);

            // Each program must be a copy, not a hard link. Gatekeeper on
            // macOS finds the path of an executed file from its inode, and
            // kills the process if that path is a link in another run's
            // directory that has since been deleted.
            std::fs::copy(&self.host, &program).map_err(io_error(&program))?;
        }

        let home = directory.path().join("home");
        std::fs::create_dir(&home).map_err(io_error(&home))?;

        if let Some(credentials) = self.credentials {
            std::os::unix::fs::chown(&home, Some(credentials.uid), Some(credentials.gid))
                .map_err(io_error(&home))?;
        }

        Ok(directory)
    }
}
