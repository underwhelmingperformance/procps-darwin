// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Takes snapshots from the fixture and local sources.

use std::{
    ffi::OsString,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime},
};

use assert_matches::assert_matches;
use darwin_proc::{Pid, ProcessInfo};
use pretty_assertions::assert_eq;
use procps_core::{
    Budget, Field, FieldGroup, FixtureSource, HelperOrLocal, LocalSource, Process, ProcessSource,
    RegionTotals, Snapshot, SnapshotRequest, Source, SourceError, Usage, helper::HelperSource,
};
use rstest::rstest;

/// The name of `field`'s variant.
const fn outcome<T>(field: &Field<T>) -> &'static str {
    match field {
        Field::Available(_) => "available",
        Field::Denied => "denied",
        Field::Unsupported => "unsupported",
        Field::Failed => "failed",
    }
}

/// The outcome of each optional group of `process`, in [`FieldGroup::ALL`]
/// order: `None` for a group that the source did not read. The usage group
/// has the outcomes of its three fields, and the regions group the outcomes of
/// the totals and the regions, separated by slashes.
fn outcomes(process: &Process) -> [Option<String>; 7] {
    fn one<T>(field: Option<&Field<T>>) -> Option<String> {
        field.map(|field| outcome(field).to_owned())
    }

    [
        one(process.arguments.as_ref()),
        one(process.environment.as_ref()),
        process.usage.as_ref().map(|usage| {
            format!(
                "{}/{}/{}",
                outcome(&usage.task),
                outcome(&usage.resources),
                outcome(&usage.counters)
            )
        }),
        one(process.threads.as_ref()),
        match (&process.region_totals, &process.regions) {
            (Some(totals), Some(regions)) => {
                Some(format!("{}/{}", outcome(totals), outcome(regions)))
            }
            (None, None) => None,
            (totals, regions) => Some(format!("{totals:?} with {regions:?}")),
        },
        one(process.file_descriptors.as_ref()),
        one(process.working_directory.as_ref()),
    ]
}

/// `outcomes` for a process in which every group has the outcome `kind`.
fn every_group(kind: &str) -> [Option<String>; 7] {
    let mut groups = [(); 7].map(|()| Some(kind.to_owned()));
    groups[2] = Some(format!("{kind}/{kind}/{kind}"));
    groups[4] = Some(format!("{kind}/{kind}"));
    groups
}

/// A request for every group of `pids`.
fn everything_for(pids: impl IntoIterator<Item = Pid>) -> SnapshotRequest {
    FieldGroup::ALL
        .into_iter()
        .fold(SnapshotRequest::default(), SnapshotRequest::with)
        .for_processes(pids)
}

/// A fixture process with `pid` and a `Field` in every group.
fn fixture_process(pid: i32) -> Result<Process, darwin_proc::Error> {
    let info = ProcessInfo {
        pid: Pid::from(pid),
        ..Pid::current().info()?
    };

    Ok(Process {
        arguments: Some(Field::Available(vec![OsString::from("sleep")])),
        environment: Some(Field::Available(Vec::new())),
        usage: Some(Usage {
            task: Field::Denied,
            resources: Field::Denied,
            counters: Field::Denied,
        }),
        threads: Some(Field::Available(Vec::new())),
        file_descriptors: Some(Field::Available(vec![0, 1, 2])),
        working_directory: Some(Field::Available(PathBuf::from("/"))),
        ..Process::from_identity(info, Field::Unsupported, Field::Available(Pid::from(pid)))
    }
    .with_regions(Field::Unsupported))
}

#[test]
fn a_fixture_returns_the_selected_processes_in_order() -> Result<(), Box<dyn std::error::Error>> {
    let taken = SystemTime::UNIX_EPOCH + Duration::from_hours(500_000);
    let [ten, twenty, thirty] = [
        fixture_process(10)?,
        fixture_process(20)?,
        fixture_process(30)?,
    ];
    let uptime = Duration::from_hours(1);
    let source = FixtureSource::new(taken, uptime, vec![thirty.clone(), ten.clone(), twenty]);
    let request = everything_for([Pid::from(30), Pid::from(10)]);

    assert_eq!(
        source.snapshot(&request)?,
        Snapshot {
            taken,
            uptime,
            processes: vec![ten, thirty],
            system: None,
        }
    );

    Ok(())
}

#[test]
fn a_fixture_leaves_out_the_groups_that_the_request_did_not_ask_for()
-> Result<(), Box<dyn std::error::Error>> {
    let source = FixtureSource::new(
        SystemTime::UNIX_EPOCH,
        Duration::ZERO,
        vec![fixture_process(10)?],
    );
    let request = SnapshotRequest::default()
        .with(FieldGroup::Arguments)
        .with(FieldGroup::Regions);
    let snapshot = source.snapshot(&request)?;
    let processes: Vec<_> = snapshot.processes.iter().map(outcomes).collect();

    assert_eq!(
        processes,
        vec![[
            Some("available".to_owned()),
            None,
            None,
            None,
            Some("unsupported/unsupported".to_owned()),
            None,
            None,
        ]]
    );

    Ok(())
}

#[test]
fn a_fixture_totals_regions_that_have_no_totals() -> Result<(), Box<dyn std::error::Error>> {
    let regions = Pid::current().regions()?;
    let process = Process {
        regions: Some(Field::Available(regions.clone())),
        ..Process::from_identity(
            Pid::current().info()?,
            Field::Unsupported,
            Field::Unsupported,
        )
    };
    let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, vec![process]);
    let request = SnapshotRequest::default().with(FieldGroup::Regions);
    let totals: Vec<_> = source
        .snapshot(&request)?
        .processes
        .into_iter()
        .map(|process| process.region_totals)
        .collect();

    assert_eq!(
        totals,
        vec![Some(Field::Available(RegionTotals::from(
            regions.as_slice()
        )))]
    );

    Ok(())
}

#[test]
fn this_process_has_a_value_in_every_group() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = LocalSource.snapshot(&everything_for([Pid::current()]))?;
    let processes: Vec<_> = snapshot
        .processes
        .iter()
        .map(|process| (process.info.pid, outcomes(process)))
        .collect();

    assert_eq!(
        (processes, snapshot.system),
        (vec![(Pid::current(), every_group("available"))], None)
    );

    Ok(())
}

#[test]
fn another_users_process_has_only_its_identity() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = LocalSource.snapshot(&everything_for([Pid::from(1)]))?;
    let processes: Vec<_> = snapshot
        .processes
        .iter()
        .map(|process| {
            (
                process.executable.clone(),
                process.session,
                outcomes(process),
            )
        })
        .collect();

    assert_eq!(
        processes,
        vec![(
            Field::Available(PathBuf::from("/sbin/launchd")),
            Field::Available(Pid::from(1)),
            every_group("denied")
        )]
    );

    Ok(())
}

#[test]
fn an_exited_process_is_left_out() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    child.wait()?;

    let snapshot = LocalSource.snapshot(&everything_for([pid]))?;

    assert_eq!(snapshot.processes, Vec::new());

    Ok(())
}

#[test]
fn a_default_snapshot_lists_every_process_with_only_its_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let snapshot = LocalSource.snapshot(&SnapshotRequest::default())?;
    let this_process = snapshot
        .processes
        .iter()
        .find(|process| process.info.pid == Pid::current())
        .map(outcomes);
    let ascending = snapshot
        .processes
        .windows(2)
        .all(|pair| pair[0].info.pid < pair[1].info.pid);

    assert_eq!(
        (this_process, ascending, snapshot.system.is_some()),
        (Some([const { None }; 7]), true, false)
    );

    Ok(())
}

#[test]
fn a_snapshot_reads_the_system_statistics_on_request() -> Result<(), Box<dyn std::error::Error>> {
    let request = SnapshotRequest::default()
        .for_processes([Pid::current()])
        .with_system();
    let system = LocalSource.snapshot(&request)?.system;

    assert_eq!(
        system.map(|system| system.boot_time),
        Some(darwin_proc::Host::boot_time()?)
    );

    Ok(())
}

#[test]
fn a_zombie_keeps_its_resource_usage() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("/usr/bin/true").spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);

    // Wait without reaping, so that the child stays a zombie.
    while pid.info()?.status != darwin_proc::Status::Zombie {
        std::thread::yield_now();
    }

    let request = SnapshotRequest::default()
        .with(FieldGroup::Usage)
        .for_processes([pid]);
    let snapshot = LocalSource.snapshot(&request);
    child.wait()?;
    let usage: Vec<_> = snapshot?
        .processes
        .iter()
        .map(|process| outcomes(process)[2].clone())
        .collect();

    assert_eq!(
        usage,
        vec![Some("unsupported/available/available".to_owned())]
    );

    Ok(())
}

/// The variable that makes [`helper_waits_until_its_input_closes`] wait.
const HELPER: &str = "PROCPS_CORE_SOURCES_HELPER";

/// Runs as the child of [`environments_are_read_or_denied`]. This test binary
/// is not an Apple platform binary, so macOS returns its environment.
#[test]
#[ignore = "runs only as a child of another test"]
fn helper_waits_until_its_input_closes() -> std::io::Result<()> {
    if std::env::var_os(HELPER).is_some() {
        std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink())?;
    }

    Ok(())
}

/// A child that waits until its standard input closes.
enum Child {
    /// `/bin/cat`, an Apple platform binary, with `argv[0]` set to `arg0`.
    PlatformBinary { arg0: &'static str },
    /// This test binary, running [`helper_waits_until_its_input_closes`].
    TestBinary,
}

/// The environment of `child`, as `LocalSource` reads it: the strings of an
/// available environment, or `None` for a denied one. Any other outcome is an
/// error.
fn environment_of(child: &Child) -> Result<Option<Vec<OsString>>, Box<dyn std::error::Error>> {
    let mut command = match child {
        Child::PlatformBinary { arg0 } => {
            let mut command = Command::new("/bin/cat");
            command
                .arg0(arg0)
                .env_clear()
                .env("FIRST", "1")
                .env("SECOND", "2");
            command
        }
        Child::TestBinary => {
            let mut command = Command::new(std::env::current_exe()?);
            command
                .args([
                    "--ignored",
                    "--exact",
                    "helper_waits_until_its_input_closes",
                ])
                .env_clear()
                .env(HELPER, "1");
            command
        }
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = Pid::from(i32::try_from(child.id())?);
    let request = SnapshotRequest::default()
        .with(FieldGroup::Environment)
        .for_processes([pid]);
    let snapshot = LocalSource.snapshot(&request);
    drop(child.stdin.take());
    child.wait()?;

    let environments: Vec<_> = snapshot?
        .processes
        .into_iter()
        .map(|process| process.environment)
        .collect();

    match &environments[..] {
        [Some(Field::Available(environment))] => Ok(Some(environment.clone())),
        [Some(Field::Denied)] => Ok(None),
        other => Err(format!("unexpected environments: {other:?}").into()),
    }
}

#[rstest]
// macOS 27 withholds the environment of Apple's platform binaries from other
// processes, and macOS 26 returns it.
#[case::platform_binary(
    Child::PlatformBinary { arg0: "cat" },
    &[None, Some(&["FIRST=1", "SECOND=2"][..])],
)]
// With an empty `argv[0]`, macOS 27 also returns the first variable, which
// `LocalSource` reports as denied.
#[case::platform_binary_with_an_empty_first_argument(
    Child::PlatformBinary { arg0: "" },
    &[None, Some(&["FIRST=1", "SECOND=2"][..])],
)]
#[case::test_binary(Child::TestBinary, &[Some(&["PROCPS_CORE_SOURCES_HELPER=1"][..])])]
fn environments_are_read_or_denied(
    #[case] child: Child,
    #[case] expected: &[Option<&[&str]>],
) -> Result<(), Box<dyn std::error::Error>> {
    let environment = environment_of(&child)?;
    let expected: Vec<Option<Vec<OsString>>> = expected
        .iter()
        .map(|strings| strings.map(|strings| strings.iter().map(OsString::from).collect()))
        .collect();

    assert!(
        expected.contains(&environment),
        "{environment:?} is not one of {expected:?}"
    );

    Ok(())
}

#[test]
fn a_fixture_reports_a_requested_group_that_it_lacks_as_unsupported()
-> Result<(), Box<dyn std::error::Error>> {
    let info = Pid::current().info()?;
    let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    let source = FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::ZERO, vec![process]);
    let request = SnapshotRequest::default()
        .with(FieldGroup::Arguments)
        .with(FieldGroup::Usage);
    let processes: Vec<_> = source
        .snapshot(&request)?
        .processes
        .iter()
        .map(outcomes)
        .collect();

    assert_eq!(
        processes,
        vec![[
            Some("unsupported".to_owned()),
            None,
            Some("unsupported/unsupported/unsupported".to_owned()),
            None,
            None,
            None,
            None,
        ]]
    );

    Ok(())
}

/// What a budget allows a snapshot.
#[derive(Clone, Copy, Debug)]
enum Allowance {
    Unlimited,
    /// A deadline that has already passed.
    NoTime,
    OneByte,
}

/// How a snapshot within a budget ended.
#[derive(Debug, PartialEq)]
enum Ending {
    Taken(Vec<Pid>),
    TimedOut,
    TooLarge(usize),
}

#[rstest]
#[case::unlimited(Allowance::Unlimited, Ending::Taken(vec![Pid::current()]))]
#[case::no_time(Allowance::NoTime, Ending::TimedOut)]
#[case::one_byte(Allowance::OneByte, Ending::TooLarge(1))]
fn a_snapshot_stays_within_its_budget(
    #[case] allowance: Allowance,
    #[case] expected: Ending,
) -> Result<(), Box<dyn std::error::Error>> {
    let budget = match allowance {
        Allowance::Unlimited => Budget::UNLIMITED,
        Allowance::NoTime => Budget::UNLIMITED.until(Instant::now()),
        Allowance::OneByte => Budget::UNLIMITED.bytes(1),
    };

    let ending = match LocalSource.snapshot_within(&everything_for([Pid::current()]), budget) {
        Ok(snapshot) => Ending::Taken(
            snapshot
                .processes
                .iter()
                .map(|process| process.info.pid)
                .collect(),
        ),
        Err(SourceError::TimedOut) => Ending::TimedOut,
        Err(SourceError::TooLarge { limit }) => Ending::TooLarge(limit),
        Err(error) => return Err(error.into()),
    };

    assert_eq!(ending, expected);

    Ok(())
}

#[rstest]
#[case::local(Source::Local(LocalSource))]
#[case::helper_or_local(Source::HelperOrLocal(HelperOrLocal::new(HelperSource::new(
    "/nonexistent/helper.sock"
))))]
fn a_source_that_reads_locally_applies_the_budget(#[case] source: Source) {
    let result = source
        .snapshot_within(
            &everything_for([Pid::current()]),
            Budget::UNLIMITED.bytes(1),
        )
        .map(|snapshot| snapshot.processes.len());

    assert_matches!(result, Err(SourceError::TooLarge { limit: 1 }));
}
