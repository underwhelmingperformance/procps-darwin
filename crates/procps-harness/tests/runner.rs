// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs scenarios against small shell scripts, using this package's binary for
//! the session leader and the fixture processes.

use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

use assert_matches::assert_matches;
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
    ("kill-leader", "kill -TERM \"$1\"\n"),
    (
        "leave-behind",
        "/bin/sleep 30 </dev/null >/dev/null 2>&1 &\necho $! >\"$1\"\n",
    ),
    ("wait-for-child", "/bin/sleep 60\n"),
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

#[rstest]
#[case::fixture(&["record", "{a1}"])]
#[case::process_left_by_the_command(&["leave-behind"])]
fn no_process_outlives_its_run(#[case] command: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let record = tools.directory.path().join("pid");
    let mut command = command.to_vec();
    command.push(record.to_str().ok_or("path is not UTF-8")?);

    tools.runner.run(&scenario(&command, FIXTURES)?)?;

    let pid = std::fs::read_to_string(&record)?;
    let alive = std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()?
        .success();

    assert!(!alive, "process {} is still running", pid.trim());

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

#[rstest]
#[case::waiting_for_a_child(&["wait-for-child"])]
#[case::stopping_the_session_leader(&["stop-leader", "{session}"])]
fn a_run_that_takes_too_long_is_stopped(
    #[case] command: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let tools = Tools::new()?;
    let timeout = std::time::Duration::from_millis(500);
    let runner = Runner::new(tools.directory.path(), env!("CARGO_BIN_EXE_procps-harness"))
        .with_timeout(timeout);

    assert_matches!(
        runner.run(&scenario(command, FIXTURES)?),
        Err(Error::Timeout { after }) if after == timeout
    );

    Ok(())
}
