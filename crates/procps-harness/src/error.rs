// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{path::PathBuf, process::ExitStatus, time::Duration};

use crate::{FixtureEnd, Outcome, Stream};

/// An error from loading, running or comparing a scenario.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read or written, or a program could not be run.
    #[error("{path}: {source}")]
    Io {
        /// The file, directory or program.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A scenario or golden file is not valid TOML for its type.
    #[error("{path}: {source}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: toml::de::Error,
    },
    /// A value could not be written as TOML.
    #[error("cannot write TOML: {0}")]
    Serialise(#[from] toml::ser::Error),
    /// A command argument refers to a pid that the run does not know.
    #[error("{argument:?} refers to an unknown pid {name:?}")]
    UnknownFixture {
        /// The argument that contains the reference.
        argument: String,
        /// The name between the braces.
        name: String,
    },
    /// The tools directory has a different procps-ng release from the golden
    /// files.
    #[error("pgrep -V reports {found:?}, but the golden files come from procps-ng {expected}")]
    WrongReference {
        /// What `pgrep -V` printed.
        found: String,
        /// The release that the golden files need.
        expected: &'static str,
    },
    /// A user and group were not given as `<uid>:<gid>`.
    #[error("{0:?} is not of the form <uid>:<gid>")]
    InvalidCredentials(String),
    /// A fixture process could not be started or waited for.
    #[error("fixture {name:?}: {source}")]
    Fixture {
        /// The fixture's name in the scenario.
        name: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A fixture process exited before it reported that it was ready.
    #[error("fixture {name:?} wrote {output:?} and exited with {status} before it was ready")]
    FixtureNotReady {
        /// The fixture's name in the scenario.
        name: String,
        /// What the fixture wrote to standard output.
        output: String,
        /// How the fixture exited.
        status: ExitStatus,
    },
    /// A fixture process exited by itself before the run stopped it.
    #[error("fixture {name:?} exited with status {status}")]
    FixtureExited {
        /// The fixture's name in the scenario.
        name: String,
        /// The raw wait status.
        status: i32,
    },
    /// A golden file contains a fixture state that the harness does not
    /// know.
    #[error("{0:?} is not a fixture state")]
    UnknownFixtureEnd(String),
    /// A command wrote something that is not UTF-8.
    #[error("the command's {stream} is not UTF-8")]
    NotUtf8 {
        /// The stream.
        stream: Stream,
    },
    /// The processes that advance the pid counter failed.
    #[error("cannot start processes to advance the pid counter: {0}")]
    SkipPids(ExitStatus),
    /// No attempt to run the scenario got pids with five digits.
    #[error(
        "cannot get pids between 10000 and 99999 for the scenario's processes; the last was {last}"
    )]
    NoFreePids {
        /// The last pid outside the range.
        last: u32,
    },
    /// The scenario's session did not finish in time, so the runner killed
    /// every process in it.
    #[error("the scenario did not finish within {after:?}")]
    Timeout {
        /// How long the scenario was allowed to run.
        after: Duration,
    },
    /// A signal terminated the session leader, probably sent by a command that
    /// signals every process in its session.
    #[error("the session leader was killed by {0}")]
    LeaderSignalled(String),
    /// The session leader could not run the scenario.
    #[error("the session leader failed: {message}")]
    Session {
        /// What the session leader reported.
        message: String,
    },
    /// The session leader's report could not be read.
    #[error("cannot read the session leader's report: {source}")]
    Report {
        /// The underlying error.
        source: toml::de::Error,
    },
    /// The command signalled a decoy, which has a fixture's name but runs
    /// outside the scenario's session.
    #[error(
        "the command left a decoy {comm:?} {end}, so it acts on processes outside the scenario's \
         session"
    )]
    DecoyDisturbed {
        /// The decoy's process name.
        comm: String,
        /// What had happened to the decoy.
        end: FixtureEnd,
    },
    /// The scenario's outcome changes when decoys run, so its command selects
    /// processes outside the scenario's session.
    #[error(
        "{tool}/{name}: the outcome changes when decoys run, so the command selects processes \
         outside the scenario's session"
    )]
    Unconfined {
        /// The scenario's tool.
        tool: String,
        /// The scenario's name.
        name: String,
        /// The outcome without decoys.
        alone: Box<Outcome>,
        /// The outcome with decoys.
        with_decoys: Box<Outcome>,
    },
}
