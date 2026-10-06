// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashSet,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread::JoinHandle,
    time::Duration,
};

use rustix::process::{Pid, Signal};
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

/// How a session leader ended.
enum SessionEnd {
    /// The leader exited before the timeout.
    Finished {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// The timeout passed, and the runner killed the session.
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

/// Reads everything from `stream` on another thread.
fn read_in_background(
    stream: Option<impl Read + Send + 'static>,
) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || read_all(stream))
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
/// When the leader exits, or the timeout passes, the runner kills every
/// process left in the session.
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
            timeout: Duration::from_secs(10),
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

    /// The runner, changed to kill every process in a scenario's session if
    /// the session leader has not finished after `timeout`. The default is ten
    /// seconds.
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
    /// finish in time, or the command signals a decoy.
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
        let text = toml::to_string(request)?;
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
            let mut child = command.spawn_alone().map_err(io_error)?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).map_err(io_error)?;
            }

            let SessionEnd::Finished {
                status,
                stdout,
                stderr,
            } = self.wait_for_session(child).map_err(io_error)?
            else {
                return Err(Error::Timeout {
                    after: self.timeout,
                });
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

    /// Waits until the session leader exits or the timeout passes, then kills
    /// every process left in its session.
    fn wait_for_session(&self, mut leader: Child) -> std::io::Result<SessionEnd> {
        let pid = Pid::from_child(&leader);
        let stdout = leader.stdout.take();
        let stderr = read_in_background(leader.stderr.take());
        let (sender, receiver) = mpsc::channel();

        // The leader's children have their own standard output, so the
        // leader's closes only when the leader exits. `waitid` cannot wait
        // for that without reaping the leader: macOS also returns when the
        // leader stops.
        std::thread::spawn(move || sender.send(read_all(stdout)));

        let finished = receiver.recv_timeout(self.timeout);

        // The leader is not reaped yet, so its pid, which is also the
        // session's process group id, cannot belong to another process.
        let _ = rustix::process::kill_process_group(pid, Signal::KILL);

        let Ok(stdout) = finished else {
            let _ = receiver.recv();
            leader.wait()?;

            return Ok(SessionEnd::TimedOut);
        };

        Ok(SessionEnd::Finished {
            status: leader.wait()?,
            stdout: stdout?,
            stderr: stderr.join().unwrap_or_else(|_| Ok(Vec::new()))?,
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
