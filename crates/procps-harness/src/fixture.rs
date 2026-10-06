// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, ExitCode, Stdio},
};

use rustix::process::{Pid, Signal, WaitOptions};

use crate::{Credentials, Error, FixtureEnd, scenario::FixtureSpec, signal, spawn::SpawnAlone};

/// The argument that makes a host binary run as a fixture process.
pub(crate) const FIXTURE_ARGUMENT: &str = "--procps-harness-fixture";

const READY: &str = "ready\n";

/// Runs as a fixture process: reports that it is ready, then waits until its
/// standard input closes.
pub(crate) fn fixture_main() -> ExitCode {
    let wait = || -> std::io::Result<u64> {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(READY.as_bytes())?;
        stdout.flush()?;

        std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink())
    };

    match wait() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Running fixture processes. Dropping this kills the ones that are still
/// running.
pub(crate) struct Fixtures(Vec<Fixture>);

struct Fixture {
    name: String,
    child: Child,
    stdin: Option<ChildStdin>,
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
            .env_clear()
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(credentials) = credentials {
            credentials.apply(&mut command);
        }

        let mut child = command.spawn_alone().map_err(failure)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let mut fixture = Self {
            name: spec.name.clone(),
            child,
            stdin,
            reaped: false,
        };

        let mut line = String::new();
        stdout
            .map(|stdout| BufReader::new(stdout).read_line(&mut line))
            .transpose()
            .map_err(failure)?;

        if line != READY {
            // The fixture closes its standard output only when it exits.
            drop(fixture.stdin.take());
            let status = fixture.child.wait().map_err(failure)?;
            fixture.reaped = true;

            return Err(Error::FixtureNotReady {
                name: spec.name.clone(),
                output: line,
                status,
            });
        }

        Ok(fixture)
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
            return Ok(FixtureEnd::Running);
        }

        Err(Error::FixtureExited {
            name: self.name.clone(),
            status: status.as_raw(),
        })
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
