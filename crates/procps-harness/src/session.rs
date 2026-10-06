// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io::{Read, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::PathBuf,
    process::{Command, ExitCode, ExitStatus, Stdio},
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Outcome, Scenario,
    fixture::Fixtures,
    outcome::Stream,
    pids::{MASKED_PIDS, PidNames},
    scenario::{COMMAND, SESSION},
    spawn::SpawnAlone,
};

/// The argument that makes a host binary run as a session leader.
pub(crate) const LEADER_ARGUMENT: &str = "--procps-harness-session";

/// The session leader's process name.
pub(crate) const LEADER_COMM: &str = "session-leader";

/// What a session leader runs, sent on its standard input.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Request {
    /// The directory that contains the scenario's tools.
    pub(crate) tools: PathBuf,
    /// The directory that contains a program for each fixture name.
    pub(crate) programs: PathBuf,
    /// The command's home and working directory.
    pub(crate) home: PathBuf,
    /// The scenario.
    pub(crate) scenario: Scenario,
}

/// What a session leader reports on its standard output.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "report", rename_all = "kebab-case")]
pub(crate) enum Report {
    /// The command finished.
    Finished {
        /// What the command did, with known pids masked.
        outcome: Outcome,
    },
    /// A process of the run got a pid outside [`MASKED_PIDS`], or a pid
    /// lower than the previous process's, so the run must start again.
    PidOutOfRange {
        /// The pid.
        pid: u32,
    },
    /// The scenario could not be run.
    Failed {
        /// Why.
        failure: Failure,
    },
}

/// An [`Error`] in a form that the session leader can report.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Failure {
    FixtureNotReady {
        name: String,
        output: String,
        status: i32,
    },
    FixtureExited {
        name: String,
        status: i32,
    },
    NotUtf8 {
        stream: Stream,
    },
    UnknownFixture {
        argument: String,
        name: String,
    },
    Other {
        message: String,
    },
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::FixtureNotReady {
                name,
                output,
                status,
            } => Self::FixtureNotReady {
                name,
                output,
                status: status.into_raw(),
            },
            Error::FixtureExited { name, status } => Self::FixtureExited { name, status },
            Error::NotUtf8 { stream } => Self::NotUtf8 { stream },
            Error::UnknownFixture { argument, name } => Self::UnknownFixture { argument, name },
            error => Self::Other {
                message: error.to_string(),
            },
        }
    }
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::FixtureNotReady {
                name,
                output,
                status,
            } => Self::FixtureNotReady {
                name,
                output,
                status: ExitStatus::from_raw(status),
            },
            Failure::FixtureExited { name, status } => Self::FixtureExited { name, status },
            Failure::NotUtf8 { stream } => Self::NotUtf8 { stream },
            Failure::UnknownFixture { argument, name } => Self::UnknownFixture { argument, name },
            Failure::Other { message } => Self::Session { message },
        }
    }
}

/// Runs as a session leader: starts a new session, runs the request's
/// scenario in it, and writes a [`Report`] to standard output.
pub(crate) fn leader_main() -> ExitCode {
    let report = lead().unwrap_or_else(|error| Report::Failed {
        failure: error.into(),
    });
    let written = toml::to_string(&report)
        .map_err(std::io::Error::other)
        .and_then(|text| std::io::stdout().lock().write_all(text.as_bytes()));

    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn lead() -> Result<Report, Error> {
    rustix::process::setsid().map_err(|source| Error::Session {
        message: format!("cannot start a session: {source}"),
    })?;

    let mut text = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|source| Error::Session {
            message: format!("cannot read the request: {source}"),
        })?;
    let request: Request = toml::from_str(&text).map_err(|source| Error::Session {
        message: format!("cannot parse the request: {source}"),
    })?;

    let session = std::process::id();
    let mut previous = session;

    if !MASKED_PIDS.contains(&session) {
        return Ok(Report::PidOutOfRange { pid: session });
    }

    let fixtures = Fixtures::start(&request.scenario.fixtures, &request.programs, None)?;
    let mut names: PidNames = fixtures.pids().collect();

    for pid in fixtures.pids().map(|(_, pid)| pid) {
        if !MASKED_PIDS.contains(&pid) || pid <= previous {
            return Ok(Report::PidOutOfRange { pid });
        }

        previous = pid;
    }

    names.extend([(SESSION.to_owned(), session)]);

    let arguments = request
        .scenario
        .command
        .iter()
        .map(|argument| argument.with_pids(&names))
        .collect::<Result<Vec<_>, _>>()?;
    let Some((program, arguments)) = arguments.split_first() else {
        return Err(Error::Session {
            message: "the scenario has no command".to_owned(),
        });
    };
    let path = request.tools.join(program);

    // The runner stops the whole session if the command runs for too long.
    let child = Command::new(&path)
        .arg0(program)
        .args(arguments)
        .env_clear()
        .env("PATH", &request.tools)
        .env("HOME", &request.home)
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .current_dir(&request.home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_alone()
        .map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
    let command = child.id();
    let output = child
        .wait_with_output()
        .map_err(|source| Error::Io { path, source })?;

    if !MASKED_PIDS.contains(&command) || command <= previous {
        return Ok(Report::PidOutOfRange { pid: command });
    }

    names.extend([(COMMAND.to_owned(), command)]);

    let outcome = Outcome {
        fixtures: fixtures.finish()?,
        ..Outcome::try_from(output)?.masked(&names)
    };

    Ok(Report::Finished { outcome })
}
