// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Withholds the values that Linux shows only to a process's owner and to
//! root.

use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use darwin_proc::{Gid, Pid, ProcessFlags, ProcessInfo, Uid};
use pretty_assertions::assert_eq;
use procps_core::{
    Field, FieldGroup, FixtureSource, LocalSource, Process, ProcessSource, Snapshot,
    SnapshotRequest, Usage, helper::Caller,
};
use rstest::rstest;

/// The user that owns the fixture process.
const OWNER: u32 = 501;

/// The group of the fixture process.
const GROUP: u32 = 20;

/// `P_SUGID`, which XNU sets when a process changes its credentials or
/// executes a set-user-ID or set-group-ID program.
const P_SUGID: u32 = 0x100;

/// A process with the real, effective and saved user IDs `users` and group
/// IDs `groups`, and a value in every field group. It has `P_SUGID` set if
/// `changed_credentials` is set.
fn process(
    users: [u32; 3],
    groups: [u32; 3],
    changed_credentials: bool,
) -> Result<Process, Box<dyn std::error::Error>> {
    let snapshot = LocalSource.snapshot(
        &FieldGroup::ALL
            .into_iter()
            .fold(SnapshotRequest::default(), SnapshotRequest::with)
            .for_processes([Pid::current()]),
    )?;
    let [mut process] = <[Process; 1]>::try_from(snapshot.processes)
        .map_err(|processes| format!("expected one process, found {processes:?}"))?;
    let flags = process.info.flags.bits() & !P_SUGID;

    process.info = ProcessInfo {
        flags: ProcessFlags::from(if changed_credentials {
            flags | P_SUGID
        } else {
            flags
        }),
        ..process.info
    };
    let credentials = &mut process.info.credentials;
    [credentials.ruid, credentials.euid, credentials.svuid] = users.map(Uid::from);
    [credentials.rgid, credentials.egid, credentials.svgid] = groups.map(Gid::from);
    process.environment = Some(Field::Available(vec![OsString::from("HOME=/")]));
    process.working_directory = Some(Field::Available(PathBuf::from("/")));

    Ok(process)
}

/// `process` without the values that only its owner and root may read.
fn withheld(process: Process) -> Process {
    Process {
        environment: Some(Field::Denied),
        usage: process.usage.map(|usage| Usage {
            counters: Field::Denied,
            ..usage
        }),
        regions: Some(Field::Denied),
        file_descriptors: Some(Field::Denied),
        working_directory: Some(Field::Denied),
        ..process
    }
}

/// A snapshot of `processes`.
fn snapshot(processes: Vec<Process>) -> Snapshot {
    Snapshot {
        taken: SystemTime::UNIX_EPOCH,
        uptime: Duration::from_secs(1),
        processes,
        system: None,
    }
}

/// A source whose processes are `processes`, for the second reading of their
/// identities.
fn later(processes: Vec<Process>) -> FixtureSource {
    FixtureSource::new(SystemTime::UNIX_EPOCH, Duration::from_secs(2), processes)
}

/// A caller with the user ID `uid` and the group ID [`GROUP`].
fn caller(uid: u32) -> Caller {
    Caller::new(Uid::from(uid), Gid::from(GROUP))
}

#[rstest]
#[case::root(0, [OWNER; 3], [GROUP; 3], false, true)]
#[case::owner(OWNER, [OWNER; 3], [GROUP; 3], false, true)]
#[case::another_user(OWNER + 1, [OWNER; 3], [GROUP; 3], false, false)]
#[case::changed_credentials(OWNER, [OWNER; 3], [GROUP; 3], true, false)]
#[case::root_and_changed_credentials(0, [OWNER; 3], [GROUP; 3], true, true)]
#[case::setuid_program(OWNER, [OWNER, 0, 0], [GROUP; 3], false, false)]
#[case::different_saved_user(OWNER, [OWNER, OWNER, 0], [GROUP; 3], false, false)]
#[case::different_real_user(OWNER, [0, OWNER, OWNER], [GROUP; 3], false, false)]
#[case::setgid_program(OWNER, [OWNER; 3], [GROUP, 0, 0], false, false)]
#[case::different_saved_group(OWNER, [OWNER; 3], [GROUP, GROUP, 0], false, false)]
#[case::different_real_group(OWNER, [OWNER; 3], [0, GROUP, GROUP], false, false)]
fn only_the_owner_and_root_read_private_values(
    #[case] uid: u32,
    #[case] users: [u32; 3],
    #[case] groups: [u32; 3],
    #[case] changed_credentials: bool,
    #[case] visible: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let process = process(users, groups, changed_credentials)?;
    let expected = if visible {
        process.clone()
    } else {
        withheld(process.clone())
    };

    assert_eq!(
        caller(uid).redact(snapshot(vec![process.clone()]), &later(vec![process]))?,
        snapshot(vec![expected])
    );

    Ok(())
}

/// How a process differs at the second reading of its identity.
#[derive(Debug)]
enum Change {
    /// It has not changed.
    None,
    /// It has executed a setuid-root program.
    SetuidProgram,
    /// It has exited.
    Exited,
    /// It has exited, and a new process has the same process ID.
    Replaced,
}

#[rstest]
#[case::unchanged(Change::None, true)]
#[case::setuid_program(Change::SetuidProgram, false)]
#[case::exited(Change::Exited, false)]
#[case::replaced(Change::Replaced, false)]
fn a_change_during_the_snapshot_withholds_private_values(
    #[case] change: Change,
    #[case] visible: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let process = process([OWNER; 3], [GROUP; 3], false)?;
    let mut changed = process.clone();
    match change {
        Change::None | Change::Exited => {}
        Change::SetuidProgram => {
            execute_setuid_root(&mut changed.info);
        }
        Change::Replaced => {
            changed.info.start_time += Duration::from_secs(1);
        }
    }
    let later = later(match change {
        Change::Exited => Vec::new(),
        Change::None | Change::SetuidProgram | Change::Replaced => vec![changed],
    });
    let expected = if visible {
        process.clone()
    } else {
        withheld(process.clone())
    };

    assert_eq!(
        caller(OWNER).redact(snapshot(vec![process]), &later)?,
        snapshot(vec![expected])
    );

    Ok(())
}

/// Changes `info` as an exec of a setuid-root program does.
fn execute_setuid_root(info: &mut ProcessInfo) {
    info.credentials.euid = Uid::from(0);
    info.credentials.svuid = Uid::from(0);
    info.flags = ProcessFlags::from(info.flags.bits() | P_SUGID);
}

#[test]
fn root_reads_private_values_without_a_second_reading() -> Result<(), Box<dyn std::error::Error>> {
    let process = process([OWNER; 3], [GROUP; 3], false)?;

    assert_eq!(
        caller(0).redact(snapshot(vec![process.clone()]), &later(Vec::new()))?,
        snapshot(vec![process])
    );

    Ok(())
}

#[test]
fn groups_that_the_request_left_out_stay_out() -> Result<(), Box<dyn std::error::Error>> {
    let info = Pid::current().info()?;
    let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);

    assert_eq!(
        caller(u32::MAX).redact(
            snapshot(vec![process.clone()]),
            &later(vec![process.clone()])
        )?,
        snapshot(vec![process])
    );

    Ok(())
}
