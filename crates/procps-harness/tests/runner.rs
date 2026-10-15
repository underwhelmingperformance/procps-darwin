// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs scenarios against small shell scripts, using this package's binary for
//! the session leader and the fixture processes. The tests read the process
//! table with darwin-proc, so they run on macOS only.
#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use assert_matches::assert_matches;
use darwin_proc::{Pid, ProcessInfo, Status};
use pretty_assertions::assert_eq;
use procps_harness::{Error, FixtureEnd, Generation, Outcome, Runner, Scenario, ScenarioFile};
use rstest::rstest;

const TOOLS: &[(&str, &str)] = &[
    (
        "find-in-session",
        "/usr/bin/pgrep -g 0 -f '^harness-alpha --'\necho \"pid $1\"\n",
    ),
    ("find-everywhere", "/usr/bin/pgrep -x harness-gamma\n"),
    ("kill-everywhere", "/usr/bin/pkill -x harness-delta\n"),
    (
        "stop-everywhere",
        "/usr/bin/pkill -STOP -x harness-epsilon\n",
    ),
    ("stop-leader", "kill -STOP \"$1\"\n"),
    ("stop-self", "kill -STOP $$\n"),
    ("kill-leader", "kill -TERM \"$1\"\n"),
    (
        "leave-behind",
        "/bin/sleep 30 </dev/null >/dev/null 2>&1 &\necho $! >\"$1\"\n",
    ),
    ("wait-for-child", "/bin/sleep 60\n"),
    ("record-and-wait", "echo $$ >\"$1\"\nexec /bin/sleep 60\n"),
    (
        "record-and-stop-leader",
        "echo $$ >\"$2\"\nkill -STOP \"$1\"\nexec /bin/sleep 60\n",
    ),
    ("parent", "echo \"$1 $PPID\"\n"),
    ("signal", "kill -TERM \"$1\"\nkill -STOP \"$2\"\n"),
    ("record", "echo \"$1\" >\"$2\"\n"),
    ("fail", "echo oops >&2\nexit 3\n"),
    ("terminate", "kill -TERM $$\n"),
];

const FIXTURES: &str = r#"
[[fixture]]
name = "a1"
comm = "harness-alpha"

[[fixture]]
name = "a2"
comm = "harness-alpha"

[[fixture]]
name = "b1"
comm = "harness-beta"
"#;

/// A directory of tools and a runner that uses them.
struct Tools {
    directory: tempfile::TempDir,
    runner: Runner,
}

impl Tools {
    fn new() -> std::io::Result<Self> {
        let directory = tempfile::tempdir()?;

        for (name, script) in TOOLS {
            let path = directory.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{script}"))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }

        let runner = Runner::new(directory.path(), env!("CARGO_BIN_EXE_procps-harness"));

        Ok(Self { directory, runner })
    }
}

fn scenario(command: &[&str], fixtures: &str) -> Result<Scenario, toml::de::Error> {
    format!("description = \"a test\"\ncommand = {command:?}\n{fixtures}").parse()
}

fn ends(entries: &[(&str, FixtureEnd)]) -> BTreeMap<String, FixtureEnd> {
    entries
        .iter()
        .map(|(name, end)| ((*name).to_owned(), end.clone()))
        .collect()
}

fn all_running() -> BTreeMap<String, FixtureEnd> {
    ends(&[
        ("a1", FixtureEnd::Running),
        ("a2", FixtureEnd::Running),
        ("b1", FixtureEnd::Running),
    ])
}

#[rstest]
#[case::fixtures_in_the_session(&["find-in-session", "{a1}"], Outcome {
    status: 0,
    stdout: "{a1}\n{a2}\npid {a1}\n".to_owned(),
    stderr: String::new(),
    fixtures: all_running(),
})]
#[case::session_leader_is_the_parent(&["parent", "{session}"], Outcome {
    status: 0,
    stdout: "{session} {session}\n".to_owned(),
    stderr: String::new(),
    fixtures: all_running(),
})]
#[case::fixture_ends(&["signal", "{a1}", "{a2}"], Outcome {
    status: 0,
    stdout: String::new(),
    stderr: String::new(),
    fixtures: ends(&[
        ("a1", FixtureEnd::Killed("SIGTERM".to_owned())),
        ("a2", FixtureEnd::Stopped("SIGSTOP".to_owned())),
        ("b1", FixtureEnd::Running),
    ]),
})]
#[case::status_and_standard_error(&["fail"], Outcome {
    status: 3,
    stdout: String::new(),
    stderr: "oops\n".to_owned(),
    fixtures: all_running(),
})]
#[case::terminating_signal(&["terminate"], Outcome {
    status: 128 + 15,
    stdout: String::new(),
    stderr: String::new(),
    fixtures: all_running(),
})]
fn a_scenario_records_what_its_command_did(
    #[case] command: &[&str],
    #[case] expected: Outcome,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;

    assert_eq!(tools.runner.run(&scenario(command, FIXTURES)?)?, expected);

    Ok(())
}

/// A temporary path that is not UTF-8, which a test cannot pass to a command.
#[derive(Debug, thiserror::Error)]
#[error("the path {0:?} is not UTF-8")]
struct PathNotUtf8(PathBuf);

/// The text of `path`, to pass to a command.
fn path_text(path: &Path) -> Result<&str, PathNotUtf8> {
    path.to_str().ok_or_else(|| PathNotUtf8(path.to_owned()))
}

/// Why a test cannot tell whether recorded processes have exited.
#[derive(Debug, thiserror::Error)]
enum RecordError {
    #[error("cannot read the recorded pid")]
    Unreadable(#[source] std::io::Error),
    #[error("the record {0:?} is not a pid")]
    NotAPid(String),
    #[error("cannot read the process table")]
    Processes(#[from] darwin_proc::Error),
}

/// The pid in the file `record`.
fn recorded_pid(record: &Path) -> Result<Pid, RecordError> {
    let text = std::fs::read_to_string(record).map_err(RecordError::Unreadable)?;
    let pid = text
        .trim()
        .parse::<i32>()
        .map_err(|_| RecordError::NotAPid(text.clone()))?;

    Ok(Pid::from(pid))
}

/// The processes that have not exited, of those that `which` selects. A
/// zombie has exited.
fn living(which: impl Fn(&ProcessInfo) -> bool) -> Result<Vec<Pid>, RecordError> {
    Ok(ProcessInfo::all()?
        .into_iter()
        .filter(|info| which(info) && info.status != Status::Zombie)
        .map(|info| info.pid)
        .collect())
}

#[rstest]
#[case::fixture(&["record", "{a1}"])]
#[case::process_left_by_the_command(&["leave-behind"])]
fn no_process_outlives_its_run(#[case] command: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let record = tools.directory.path().join("pid");
    let mut command = command.to_vec();
    command.push(path_text(&record)?);

    tools.runner.run(&scenario(&command, FIXTURES)?)?;
    let pid = recorded_pid(&record)?;

    assert_eq!(living(|info| info.pid == pid)?, []);

    Ok(())
}

#[rstest]
#[case::timed_out(
    &["record-and-wait"],
    Some(Duration::from_secs(30)),
    |error: &Error| matches!(error, Error::Timeout { .. })
)]
#[case::stopped_leader(
    &["record-and-stop-leader", "{session}"],
    None,
    |error: &Error| matches!(error, Error::LeaderStopped(signal) if signal == "SIGSTOP")
)]
fn the_command_does_not_outlive_a_run_that_fails(
    #[case] command: &[&str],
    #[case] timeout: Option<Duration>,
    #[case] expected: fn(&Error) -> bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    // The command has to record its pid before the timeout passes.
    let runner = match timeout {
        Some(timeout) => tools.runner.clone().with_timeout(timeout),
        None => tools.runner.clone(),
    };
    let record = tools.directory.path().join("pid");
    let mut command = command.to_vec();
    command.push(path_text(&record)?);

    assert_matches!(
        run_or_give_up(runner, scenario(&command, FIXTURES)?)?,
        Err(error) if expected(&error)
    );
    let pid = recorded_pid(&record)?;

    assert_eq!(living(|info| info.pid == pid)?, []);

    Ok(())
}

// The next tests run commands that act outside their session, so each uses a
// process name that no other test uses.
const GAMMA: &str = r#"
[[fixture]]
name = "g1"
comm = "harness-gamma"
"#;

const DELTA: &str = r#"
[[fixture]]
name = "d1"
comm = "harness-delta"
"#;

const EPSILON: &str = r#"
[[fixture]]
name = "e1"
comm = "harness-epsilon"
"#;

#[rstest]
#[case::killed(
    "kill-everywhere",
    DELTA,
    "harness-delta",
    FixtureEnd::Killed("SIGTERM".to_owned())
)]
#[case::stopped(
    "stop-everywhere",
    EPSILON,
    "harness-epsilon",
    FixtureEnd::Stopped("SIGSTOP".to_owned())
)]
fn a_command_that_signals_a_decoy_is_an_error(
    #[case] tool: &str,
    #[case] fixtures: &str,
    #[case] expected_comm: &str,
    #[case] expected_end: FixtureEnd,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;

    assert_matches!(
        tools.runner.run(&scenario(&[tool], fixtures)?),
        Err(Error::DecoyDisturbed { comm, end })
            if comm == expected_comm && end == expected_end
    );

    Ok(())
}

#[test]
fn a_command_that_kills_the_session_leader_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;

    assert_matches!(
        tools
            .runner
            .run(&scenario(&["kill-leader", "{session}"], FIXTURES)?),
        Err(Error::LeaderSignalled(signal)) if signal == "SIGTERM"
    );

    Ok(())
}

#[test]
fn generation_rejects_a_scenario_that_sees_decoys() -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let golden = tempfile::tempdir()?;
    let file = ScenarioFile {
        tool: "find-everywhere".to_owned(),
        name: "unconfined".to_owned(),
        scenario: scenario(&["find-everywhere"], GAMMA)?,
    };

    assert_matches!(
        tools.runner.generate(&[file], golden.path()),
        Err(Error::Unconfined { tool, name, .. })
            if tool == "find-everywhere" && name == "unconfined"
    );

    Ok(())
}

#[test]
fn generation_writes_golden_files_and_removes_stale_ones() -> Result<(), Box<dyn std::error::Error>>
{
    let tools = Tools::new()?;
    let golden = tempfile::tempdir()?;
    let stale = golden.path().join("fail").join("removed.toml");
    std::fs::create_dir_all(golden.path().join("fail"))?;
    std::fs::write(&stale, "")?;
    let file = ScenarioFile {
        tool: "fail".to_owned(),
        name: "status".to_owned(),
        scenario: scenario(&["fail"], "")?,
    };
    let written = golden.path().join("fail").join("status.toml");

    let generation = tools.runner.generate(&[file], golden.path())?;

    assert_eq!(
        (generation, Outcome::read(&written)?),
        (
            Generation {
                written: vec![written.clone()],
                removed: vec![stale],
            },
            Outcome {
                status: 3,
                stdout: String::new(),
                stderr: "oops\n".to_owned(),
                fixtures: BTreeMap::new(),
            }
        )
    );

    Ok(())
}

fn pgrep_reporting(version: &str) -> std::io::Result<(tempfile::TempDir, Runner)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("pgrep");
    std::fs::write(&path, format!("#!/bin/sh\necho '{version}'\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    let runner = Runner::new(directory.path(), env!("CARGO_BIN_EXE_procps-harness"));

    Ok((directory, runner))
}

#[test]
fn the_reference_release_is_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let (_directory, runner) = pgrep_reporting("pgrep from procps-ng 4.0.7")?;

    assert_matches!(runner.require_reference(), Ok(()));

    Ok(())
}

#[test]
fn another_release_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let (_directory, runner) = pgrep_reporting("pgrep from procps-ng 4.0.6")?;

    assert_matches!(
        runner.require_reference(),
        Err(Error::WrongReference { found, .. }) if found == "pgrep from procps-ng 4.0.6"
    );

    Ok(())
}

/// Why [`kill_children`] could not kill the children of the test process.
#[derive(Debug, thiserror::Error)]
enum KillError {
    #[error("cannot read the process table")]
    Processes(#[from] RecordError),
    #[error("cannot kill process {pid} or the process group that it leads")]
    Kill {
        pid: Pid,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the process group of {pid} still had members after the test had sent SIGKILL to it for \
         {GROUP_KILL_LIMIT:?}"
    )]
    Survived { pid: Pid },
}

/// A run that [`run_or_give_up`] stopped waiting for.
#[derive(Debug, thiserror::Error)]
enum GaveUp {
    /// The test killed these children of the test process, which are the
    /// session leaders and decoys of the runs, and the process groups that
    /// they lead.
    #[error(
        "the run did not return within two minutes; the test killed these children of the test \
         process and the process groups that they lead: {0:?}"
    )]
    Killed(Vec<Pid>),
    #[error("the run did not return within two minutes, and the test could not kill its children")]
    NotKilled(#[from] KillError),
    #[error("the thread of the run ended without returning the run's outcome")]
    RunLost,
}

/// How long [`kill_children`] goes on killing a process group whose members are
/// still alive.
const GROUP_KILL_LIMIT: Duration = Duration::from_secs(10);

/// Sends `SIGKILL` to `pid` and to the process group that it leads. XNU's
/// `killpg` misses a child that a member forks during the call, so the group
/// kill is repeated until `killpg` fails with `ESRCH`, when the group does not
/// exist, or with `EPERM`, once only zombies remain. If members are still alive
/// after [`GROUP_KILL_LIMIT`], the function returns [`KillError::Survived`].
fn kill_with_group(pid: Pid) -> Result<(), KillError> {
    let failed = |source: rustix::io::Errno| KillError::Kill {
        pid,
        source: source.into(),
    };
    let Some(child) = rustix::process::Pid::from_raw(pid.as_raw()) else {
        return Ok(());
    };
    let deadline = Instant::now() + GROUP_KILL_LIMIT;

    loop {
        match rustix::process::kill_process_group(child, rustix::process::Signal::KILL) {
            Ok(()) => {}
            Err(rustix::io::Errno::SRCH | rustix::io::Errno::PERM) => break,
            Err(errno) => return Err(failed(errno)),
        }

        if Instant::now() >= deadline {
            return Err(KillError::Survived { pid });
        }

        std::thread::yield_now();
    }

    match rustix::process::kill_process(child, rustix::process::Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(errno) => Err(failed(errno)),
    }
}

/// Kills each living child of the test process, and the process group that it
/// leads, and returns their pids. A session leader leads a group with its
/// fixtures and its command, and a decoy leads no group. This also ends the
/// runs of other tests, which then fail. When the kill of one child fails, the
/// function still kills the others, and then returns the first failure.
fn kill_children() -> Result<Vec<Pid>, KillError> {
    let myself = Pid::current();
    let children = living(|info| info.ppid == myself)?;
    let mut first_failure = None;

    for &child in &children {
        if let Err(failure) = kill_with_group(child) {
            first_failure.get_or_insert(failure);
        }
    }

    match first_failure {
        Some(failure) => Err(failure),
        None => Ok(children),
    }
}

/// What `runner` reports for `scenario`. Returns [`GaveUp`] if the run has not
/// returned after two minutes, or if its thread ends without returning the
/// outcome. The test then fails.
fn run_or_give_up(runner: Runner, scenario: Scenario) -> Result<Result<Outcome, Error>, GaveUp> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || sender.send(runner.run(&scenario)));

    match receiver.recv_timeout(Duration::from_secs(120)) {
        Ok(outcome) => Ok(outcome),
        Err(RecvTimeoutError::Timeout) => Err(GaveUp::Killed(kill_children()?)),
        Err(RecvTimeoutError::Disconnected) => Err(GaveUp::RunLost),
    }
}

#[test]
fn a_command_that_stops_the_session_leader_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;

    assert_matches!(
        run_or_give_up(tools.runner, scenario(&["stop-leader", "{session}"], FIXTURES)?)?,
        Err(Error::LeaderStopped(signal)) if signal == "SIGSTOP"
    );

    Ok(())
}

#[rstest]
#[case::waiting_for_a_child(&["wait-for-child"], Duration::from_millis(500))]
// A timeout of zero normally passes before the session leader calls `setsid`.
// The command stops itself, so only the timeout can end the run.
#[case::before_the_session_starts(&["stop-self"], Duration::ZERO)]
fn a_run_that_takes_too_long_is_stopped(
    #[case] command: &[&str],
    #[case] timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let runner = Runner::new(tools.directory.path(), env!("CARGO_BIN_EXE_procps-harness"))
        .with_timeout(timeout);

    assert_matches!(
        run_or_give_up(runner, scenario(command, FIXTURES)?)?,
        Err(Error::Timeout { after }) if after == timeout
    );

    Ok(())
}

/// A request size larger than 64 KiB, the largest buffer that a macOS pipe
/// uses. The runner's write of such a request is still going on when the
/// leader exits or stops without reading it.
const LARGER_THAN_A_PIPE: usize = 200_000;

/// A runner whose session leader is `script`, a shell script.
fn runner_with_leader(tools: &Tools, script: &str) -> std::io::Result<Runner> {
    let host = tools.directory.path().join("host");
    std::fs::write(&host, format!("#!/bin/sh\n{script}"))?;
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755))?;

    Ok(Runner::new(tools.directory.path(), host))
}

#[test]
fn a_leader_that_exits_before_it_reads_the_request_reports_why()
-> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let runner = runner_with_leader(&tools, "echo gone >&2\nexit 3\n")?;
    let large = "x".repeat(LARGER_THAN_A_PIPE);

    assert_matches!(
        run_or_give_up(runner, scenario(&["fail", &large], "")?)?,
        Err(Error::Session { message }) if message == "exit status: 3: gone"
    );

    Ok(())
}

// The runner writes the request on a separate thread. A write on the runner's
// thread would block once the pipe is full, before the runner sees the stop.
#[test]
fn a_leader_that_stops_before_it_reads_the_request_is_reported()
-> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let runner = runner_with_leader(&tools, "kill -STOP $$\n")?;
    let large = "x".repeat(LARGER_THAN_A_PIPE);

    assert_matches!(
        run_or_give_up(runner, scenario(&["fail", &large], "")?)?,
        Err(Error::LeaderStopped(signal)) if signal == "SIGSTOP"
    );

    Ok(())
}
