// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{path::PathBuf, process::ExitStatus, sync::Arc, time::Duration};

use crate::{FixtureEnd, FixtureState, Outcome, Stream};

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
        /// The fixture's name in the scenario, or the process name when the
        /// runner prepares a program.
        name: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A fixture process exited before it reported that it was ready.
    #[error("fixture {name:?} wrote {output:?} and exited with {status} before it was ready")]
    FixtureNotReady {
        /// The fixture's name in the scenario, or the process name when the
        /// runner prepares a program.
        name: String,
        /// What the fixture wrote to standard output.
        output: String,
        /// How the fixture exited.
        status: ExitStatus,
    },
    /// A fixture process did not report that it was ready within `after`, so
    /// the runner killed it.
    #[error(
        "fixture {name:?} was not ready within {after:?}; it {state}, and it had written {output:?}"
    )]
    FixtureNotReadyInTime {
        /// The fixture's name in the scenario, or the process name when the
        /// runner prepares a program.
        name: String,
        /// How long the runner waited.
        after: Duration,
        /// What the fixture was doing when the runner stopped waiting.
        state: FixtureState,
        /// What the fixture had written to standard output by then.
        output: String,
    },
    /// A fixture process exited by itself before the run stopped it.
    #[error("fixture {name:?} exited with status {status}")]
    FixtureExited {
        /// The fixture's name in the scenario.
        name: String,
        /// The raw wait status.
        status: i32,
    },
    /// The runner could not prepare the program for a process name before the
    /// program's first use, so no run of the runner or its clones can start the
    /// program.
    #[error("cannot prepare {}: {source}", program.display())]
    ProgramNotReady {
        /// The program.
        program: PathBuf,
        /// Why the preparation failed. Every run that needs the program
        /// returns the same error.
        source: Arc<Error>,
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
    /// The session leader neither exited nor stopped in time, so the runner
    /// sent `SIGKILL` to the leader and its process group.
    #[error("the scenario did not finish within {after:?}")]
    Timeout {
        /// How long the scenario was allowed to run.
        after: Duration,
    },
    /// The runner's [`Expiry`](crate::Expiry) ended the wait for the session
    /// leader, so the runner sent `SIGKILL` to the leader and its process
    /// group.
    #[error("the runner's expiry ended the scenario")]
    Expired,
    /// A signal terminated the session leader, probably sent by a command that
    /// signals every process in its session.
    #[error("the session leader was killed by {0}")]
    LeaderSignalled(String),
    /// A signal stopped the session leader. The scenario's command probably
    /// signalled every process in the session.
    #[error("the session leader was stopped by {0}")]
    LeaderStopped(String),
    /// The runner could not start a thread that writes the request to the
    /// session leader, reads the leader's output or waits for the leader.
    #[error("cannot start a thread for the session leader: {0}")]
    WatchThread(#[source] std::io::Error),
    /// Waiting for the session leader to exit or stop failed.
    #[error("cannot wait for the session leader: {0}")]
    WaitLeader(#[source] std::io::Error),
    /// The thread that waits for the session leader ended without a result.
    #[error("the thread that waits for the session leader ended without a result")]
    WaiterLost,
    /// Reading the session leader's standard output or standard error failed.
    #[error("cannot read the session leader's output: {0}")]
    ReadLeader(#[source] std::io::Error),
    /// A thread that reads the session leader's output panicked.
    #[error("a thread that reads the session leader's output panicked")]
    ReaderPanicked,
    /// The runner could not kill the session leader.
    #[error("cannot kill the session leader: {0}")]
    KillLeader(#[source] std::io::Error),
    /// The runner could not kill the session leader's process group.
    #[error("cannot kill the session leader's process group: {0}")]
    KillGroup(#[source] std::io::Error),
    /// Members of the session leader's process group were still alive after
    /// the runner had sent `SIGKILL` to the group repeatedly for `after`.
    #[error(
        "process group {group} still had members after the runner sent SIGKILL to it repeatedly for {after:?}"
    )]
    GroupSurvived {
        /// The process group ID.
        group: i32,
        /// How long the runner sent `SIGKILL` to the group for.
        after: std::time::Duration,
    },
    /// The runner could not send the request to the session leader.
    #[error("cannot send the request to the session leader: {0}")]
    WriteRequest(#[source] std::io::Error),
    /// The thread that sends the request to the session leader panicked.
    #[error("the thread that sends the request to the session leader panicked")]
    WriterPanicked,
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
    #[error("the command signalled a decoy {comm:?} outside the scenario's session: {end}")]
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
