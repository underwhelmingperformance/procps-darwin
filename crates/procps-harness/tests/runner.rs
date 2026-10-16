// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs scenarios against small shell scripts, using this package's binary for
//! the session leader and the fixture processes. The tests read the process
//! table with darwin-proc, so they run on macOS only.
#![cfg(target_os = "macos")]

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    os::{fd::OwnedFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use assert_matches::assert_matches;
use darwin_proc::{Pid, ProcessInfo, Status};
use pretty_assertions::assert_eq;
use procps_harness::{
    Error, Expiry, FixtureEnd, Generation, Outcome, Runner, Scenario, ScenarioFile,
};
use rstest::rstest;

const TOOLS: &[(&str, &str)] = &[
    (
        "find-in-session",
        "/usr/bin/pgrep -g 0 -f '^harness-alpha --'\necho \"pid $1\"\n",
    ),
    ("find-everywhere", "/usr/bin/pgrep -x harness-gamma\n"),
    ("kill-everywhere", "/usr/bin/pkill -x harness-delta\n"),
    (
        "hang-up-everywhere",
        "/usr/bin/pkill -HUP -x harness-zeta\n",
    ),
    (
        "stop-everywhere",
        "/usr/bin/pkill -STOP -x harness-epsilon\n",
    ),
    ("stop-leader", "kill -STOP \"$1\"\n"),
    ("stop-self", "kill -STOP $$\n"),
    // The subshell keeps forking, and it writes a line to the FIFO `$3` once
    // its loop has started. The command stops the leader only after it has read
    // that line.
    (
        "fork-and-stop-leader",
        "echo \"$1\" >\"$2\"\n\
         (while :; do /bin/sleep 60 & p=$!; [ -n \"$q\" ] && kill \"$q\"; q=$p; \
         [ -n \"$told\" ] || { told=1; echo >\"$3\"; }; done) &\n\
         read line <\"$3\"\n\
         kill -STOP \"$1\"\nwait\n",
    ),
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
    (
        "signal",
        "kill -TERM \"$1\"\nkill -STOP \"$2\"\nkill -HUP \"$3\"\n",
    ),
    ("record", "echo \"$1\" >\"$2\"\n"),
    (
        "record-and-wait-for",
        "echo \"$1\" >\"$2\"\nread line <\"$3\"\n",
    ),
    ("fail", "echo oops >&2\nexit 3\n"),
    (
        "environment",
        "echo \"LANG=$LANG LC_ALL=$LC_ALL LC_CTYPE=$LC_CTYPE TZ=$TZ\"\n",
    ),
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
catch = ["SIGHUP"]
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
#[case::fixture_ends(&["signal", "{a1}", "{a2}", "{b1}"], Outcome {
    status: 0,
    stdout: String::new(),
    stderr: String::new(),
    fixtures: ends(&[
        ("a1", FixtureEnd::Killed("SIGTERM".to_owned())),
        ("a2", FixtureEnd::Stopped("SIGSTOP".to_owned())),
        ("b1", FixtureEnd::Caught(vec!["SIGHUP".to_owned()])),
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

#[rstest]
#[case::defaults("", "LANG= LC_ALL=C LC_CTYPE= TZ=UTC\n")]
#[case::all_categories(
    "LANG = \"POSIX\"\nLC_ALL = \"C.UTF-8\"\nTZ = \"JST-9\"",
    "LANG=POSIX LC_ALL=C.UTF-8 LC_CTYPE= TZ=JST-9\n"
)]
#[case::one_category("LC_CTYPE = \"C.UTF-8\"", "LANG= LC_ALL= LC_CTYPE=C.UTF-8 TZ=UTC\n")]
fn a_scenario_sets_variables_for_its_command(
    #[case] table: &str,
    #[case] stdout: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let environment = format!("[environment]\n{table}\n{FIXTURES}");

    assert_eq!(
        tools
            .runner
            .run(&scenario(&["environment"], &environment)?)?,
        Outcome {
            status: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
            fixtures: all_running(),
        }
    );

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
    parse_pid(&std::fs::read_to_string(record).map_err(RecordError::Unreadable)?)
}

/// The pid in `text`, which a command recorded.
fn parse_pid(text: &str) -> Result<Pid, RecordError> {
    let pid = text
        .trim()
        .parse::<i32>()
        .map_err(|_| RecordError::NotAPid(text.to_owned()))?;

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

// These tests check that no process in the run's process group is alive when
// `run` returns.

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

#[test]
fn the_command_does_not_outlive_a_run_whose_leader_stops() -> Result<(), Box<dyn std::error::Error>>
{
    let tools = Tools::new()?;
    let record = tools.directory.path().join("pid");
    let command = ["record-and-stop-leader", "{session}", path_text(&record)?];

    assert_matches!(
        run_or_give_up(tools.runner.clone(), scenario(&command, FIXTURES)?)?,
        Err(Error::LeaderStopped(signal)) if signal == "SIGSTOP"
    );
    let pid = recorded_pid(&record)?;

    assert_eq!(living(|info| info.pid == pid)?, []);

    Ok(())
}

/// The thread that ran a scenario panicked.
#[derive(Debug, thiserror::Error)]
#[error("the thread that ran the scenario panicked")]
struct RunPanicked;

// The command writes its pid to a FIFO and then waits. The test reads the pid
// and then ends the run's wait, and the runner kills the run's processes as it
// does after a timeout. The test opens both ends of the FIFO before the run
// starts, and the run's thread keeps the write end until the run returns. A run
// that fails before the command writes its pid therefore ends the test's read.
#[test]
fn the_command_does_not_outlive_a_run_that_expires() -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let record = tools.directory.path().join("pid");
    nix::unistd::mkfifo(
        &record,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )?;
    // rustix does not add `CLOEXEC` as std does. Without it, the children of
    // concurrent tests would inherit a write end and keep the read open.
    let reader = rustix::fs::open(
        &record,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    rustix::fs::fcntl_setfl(&reader, rustix::fs::OFlags::empty())?;
    let writer = rustix::fs::open(
        &record,
        rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let expiry = Expiry::default();
    let runner = tools.runner.clone().with_expiry(expiry.clone());
    let scenario = scenario(&["record-and-wait", path_text(&record)?], FIXTURES)?;
    let run = std::thread::spawn(move || {
        let outcome = run_or_give_up(runner, scenario);
        drop(writer);

        outcome
    });

    let mut recorded = String::new();
    let read = BufReader::new(std::fs::File::from(reader)).read_line(&mut recorded);
    expiry.expire();
    let outcome = run.join().map_err(|_| RunPanicked)?;

    assert_matches!(outcome?, Err(Error::Expired));
    read?;
    let pid = parse_pid(&recorded)?;

    assert_eq!(living(|info| info.pid == pid)?, []);

    Ok(())
}

#[test]
fn a_run_with_an_expiry_that_has_expired_ends_at_once() -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let expiry = Expiry::default();
    expiry.expire();
    let runner = tools.runner.clone().with_expiry(expiry);

    assert_matches!(
        run_or_give_up(runner, scenario(&["wait-for-child"], FIXTURES)?)?,
        Err(Error::Expired)
    );

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

const ZETA: &str = r#"
[[fixture]]
name = "z1"
comm = "harness-zeta"
catch = ["SIGHUP"]
"#;

const ETA: &str = r#"
[[fixture]]
name = "h1"
comm = "harness-eta"
threads = 2
catch = ["SIGUSR1"]
"#;

// The command writes the fixture's pid to a FIFO and waits on a second FIFO,
// so the test reads the fixture's threads and handlers while the command runs.
#[test]
fn a_fixture_has_its_threads_and_handlers_when_the_command_starts()
-> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let record = tools.directory.path().join("pid");
    let release = tools.directory.path().join("release");

    for fifo in [&record, &release] {
        nix::unistd::mkfifo(
            fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )?;
    }

    let scenario = scenario(
        &[
            "record-and-wait-for",
            "{h1}",
            record.to_str().ok_or("path is not UTF-8")?,
            release.to_str().ok_or("path is not UTF-8")?,
        ],
        ETA,
    )?;
    let runner = tools.runner.clone();
    let unblock = record.clone();
    let run = std::thread::spawn(move || {
        let outcome = run_or_give_up(runner, scenario);

        // If the command never wrote the pid, the test is still reading the
        // FIFO, and opening the FIFO for writing ends that read.
        let _ = rustix::fs::open(
            &unblock,
            rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        );

        outcome
    });

    let pid = recorded_pid(&record)?;
    let fixture = (
        pid.task_info()?.threads,
        pid.info()?
            .caught_signals
            .contains(rustix::process::Signal::USR1.as_raw()),
    );
    std::fs::write(&release, "\n")?;
    let outcome = run
        .join()
        .map_err(|_| "the thread of the run panicked")???;

    assert_eq!(
        (fixture, outcome),
        (
            (3, true),
            Outcome {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
                fixtures: ends(&[("h1", FixtureEnd::Running)]),
            }
        )
    );

    Ok(())
}

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
#[case::caught(
    "hang-up-everywhere",
    ZETA,
    "harness-zeta",
    FixtureEnd::Caught(vec!["SIGHUP".to_owned()])
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

// XNU's `killpg` misses a child that a member forks during the call. The
// command keeps forking while the runner kills the leader's process group, so
// a single kill leaves a process in the group in some runs.
#[test]
fn no_process_outlives_a_run_whose_command_keeps_forking() -> Result<(), Box<dyn std::error::Error>>
{
    let tools = Tools::new()?;
    let record = tools.directory.path().join("session");
    let forking = tools.directory.path().join("forking");
    nix::unistd::mkfifo(
        &forking,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )?;
    let command = [
        "fork-and-stop-leader",
        "{session}",
        path_text(&record)?,
        path_text(&forking)?,
    ];

    for _ in 0..5 {
        assert_matches!(
            run_or_give_up(tools.runner.clone(), scenario(&command, FIXTURES)?)?,
            Err(Error::LeaderStopped(signal)) if signal == "SIGSTOP"
        );
        let session = recorded_pid(&record)?;

        assert_eq!(living(|info| info.pgid == session)?, []);
    }

    Ok(())
}

/// A request size larger than 64 KiB, the largest buffer that a macOS pipe
/// uses. The runner's write of such a request is still going on when the
/// leader exits or stops without reading it.
const LARGER_THAN_A_PIPE: usize = 200_000;

/// A runner whose session leader is `script`, a shell script. A process that
/// the runner starts as a fixture reports that it is ready and exits.
fn runner_with_leader(tools: &Tools, script: &str) -> Result<Runner, Box<dyn std::error::Error>> {
    runner_with_host(tools, "echo ready\nexit\n", script)
}

/// A runner whose host is a shell script. Each process that runs the script
/// first appends a line to [`STARTS`], with the path that it runs from and its
/// first argument. A process that the runner starts as a fixture then runs
/// `fixture`, and a session leader runs `leader`.
fn runner_with_host(
    tools: &Tools,
    fixture: &str,
    leader: &str,
) -> Result<Runner, Box<dyn std::error::Error>> {
    let host = tools.directory.path().join("host");
    let starts = path_text(&tools.directory.path().join(STARTS))?.to_owned();
    let script = format!(
        "#!/bin/sh\necho \"$0 $1\" >>'{starts}'\nif [ \"$1\" = --procps-harness-fixture ]; \
         then\n{fixture}fi\n{leader}"
    );
    std::fs::write(&host, script)?;
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755))?;

    Ok(Runner::new(tools.directory.path(), host))
}

/// The file in the tools directory where [`runner_with_host`]'s script
/// records each start.
const STARTS: &str = "starts";

/// The starts that [`runner_with_host`]'s script recorded, each as the path
/// that it ran from and its first argument.
fn starts(tools: &Tools) -> std::io::Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(tools.directory.path().join(STARTS))?;

    Ok(text
        .lines()
        .map(|line| {
            let (path, argument) = line.rsplit_once(' ').unwrap_or((line, ""));
            (path.to_owned(), argument.to_owned())
        })
        .collect())
}

#[test]
fn a_runner_runs_each_program_once_and_keeps_it_until_it_is_dropped()
-> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let runner = runner_with_leader(&tools, "echo gone >&2\nexit 3\n")?;
    let scenario = scenario(&["fail"], "")?;

    let outcomes = [runner.run(&scenario), runner.run(&scenario)]
        .map(|outcome| outcome.map_err(|error| error.to_string()));
    drop(runner);
    let starts = starts(&tools)?;
    let program = starts
        .first()
        .map(|(path, _)| path.clone())
        .unwrap_or_default();
    let start = |argument: &str| (program.clone(), argument.to_owned());

    assert_eq!(
        (
            outcomes,
            starts,
            Path::new(&program).file_name(),
            Path::new(&program).exists(),
        ),
        (
            [
                Err("the session leader failed: exit status: 3: gone".to_owned()),
                Err("the session leader failed: exit status: 3: gone".to_owned()),
            ],
            vec![
                start("--procps-harness-fixture"),
                start("--procps-harness-session"),
                start("--procps-harness-session"),
            ],
            Some(std::ffi::OsStr::new("session-leader")),
            false,
        )
    );

    Ok(())
}

// The host is read-only, as files in the Nix store are. Copying it again over
// the first copy would fail with EACCES, so a run that prepared the program
// again would report a different error.
#[test]
fn a_failed_preparation_fails_every_run_with_its_error() -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let runner = runner_with_host(&tools, "echo nope\nexit 1\n", "exit 3\n")?;
    std::fs::set_permissions(
        tools.directory.path().join("host"),
        std::fs::Permissions::from_mode(0o555),
    )?;
    let scenario = scenario(&["fail"], "")?;

    let outcomes = [runner.run(&scenario), runner.run(&scenario)]
        .map(|outcome| outcome.map_err(|error| error.to_string()));
    let starts = starts(&tools)?;
    let program = starts
        .first()
        .map(|(path, _)| path.clone())
        .unwrap_or_default();
    let failure = format!(
        "cannot prepare {program}: fixture \"session-leader\" wrote \"nope\\n\" and exited with \
         exit status: 1 before it was ready"
    );

    assert_eq!(
        (outcomes, starts),
        (
            [Err(failure.clone()), Err(failure)],
            vec![(program.clone(), "--procps-harness-fixture".to_owned())],
        )
    );

    Ok(())
}

/// A scenario whose one fixture has the process name `comm`.
fn scenario_with_fixture(comm: &str) -> Result<Scenario, toml::de::Error> {
    scenario(
        &["fail"],
        &format!("[[fixture]]\nname = \"f1\"\ncomm = \"{comm}\"\n"),
    )
}

/// The read and write ends of the FIFO at `path`, opened without blocking.
fn open_fifo(path: &Path) -> rustix::io::Result<(OwnedFd, OwnedFd)> {
    let reader = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    rustix::fs::fcntl_setfl(&reader, rustix::fs::OFlags::empty())?;
    let writer = rustix::fs::open(
        path,
        rustix::fs::OFlags::WRONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;

    Ok((reader, writer))
}

// The first run's thread keeps a write end of `waiting` until the run returns,
// so the test's read returns at end of file if the run fails before the
// preparation of harness-slow opens the FIFO. The test keeps its read end of
// `waiting` and both ends of `release` until the end, so the script can open
// each FIFO without waiting for the test.
#[test]
fn programs_are_prepared_in_parallel_and_once_for_each_name()
-> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let waiting = tools.directory.path().join("waiting");
    let release = tools.directory.path().join("release");

    for fifo in [&waiting, &release] {
        nix::unistd::mkfifo(
            fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )?;
    }

    let fixture = format!(
        "if [ \"${{0##*/}}\" = harness-slow ]; then\necho waiting >'{waiting}'\nread line \
         <'{release}'\necho \"$line\"\nexit\nfi\necho ready\nexit\n",
        waiting = path_text(&waiting)?,
        release = path_text(&release)?,
    );
    let runner = runner_with_host(&tools, &fixture, "echo gone >&2\nexit 3\n")?;
    let (reader, writer) = open_fifo(&waiting)?;
    let (_release_reader, release_writer) = open_fifo(&release)?;

    let slow = scenario_with_fixture("harness-slow")?;
    let first = {
        let (runner, slow) = (runner.clone(), slow.clone());
        std::thread::spawn(move || {
            let outcome = runner.run(&slow);
            drop(writer);
            outcome
        })
    };
    let mut line = String::new();
    let mut waiting_reader = BufReader::new(std::fs::File::from(reader));
    waiting_reader.read_line(&mut line)?;
    let second = {
        let runner = runner.clone();
        std::thread::spawn(move || runner.run(&slow))
    };
    let quick = run_or_give_up(runner.clone(), scenario_with_fixture("harness-quick")?)?;
    let mut release_writer = std::fs::File::from(release_writer);
    release_writer.write_all(b"nope\n")?;
    let outcomes = [
        first.join().map_err(|_| RunPanicked)?,
        second.join().map_err(|_| RunPanicked)?,
        quick,
    ]
    .map(|outcome| outcome.map_err(|error| error.to_string()));

    let starts = starts(&tools)?;
    let slow_program = starts
        .iter()
        .find(|(path, _)| path.ends_with("/harness-slow"))
        .map(|(path, _)| path.clone())
        .unwrap_or_default();
    let mut names: Vec<_> = starts
        .iter()
        .map(|(path, argument)| {
            let name = Path::new(path).file_name().unwrap_or_default();
            (name.to_string_lossy().into_owned(), argument.clone())
        })
        .collect();
    names.sort();
    let failure = format!(
        "cannot prepare {slow_program}: fixture \"harness-slow\" wrote \"nope\\n\" and exited \
         with exit status: 0 before it was ready"
    );
    let start = |name: &str, argument: &str| (name.to_owned(), argument.to_owned());

    assert_eq!(
        (line, outcomes, names),
        (
            "waiting\n".to_owned(),
            [
                Err(failure.clone()),
                Err(failure),
                Err("the session leader failed: exit status: 3: gone".to_owned()),
            ],
            vec![
                start("harness-quick", "--procps-harness-fixture"),
                start("harness-quick", "--procps-harness-fixture"),
                start("harness-slow", "--procps-harness-fixture"),
                start("session-leader", "--procps-harness-fixture"),
                start("session-leader", "--procps-harness-session"),
            ],
        )
    );

    Ok(())
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
