// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Maps Darwin's memory regions and task sizes onto procps-ng's size columns,
//! and Darwin's command names onto Linux's.

use std::time::Duration;

use darwin_proc::{Pid, Protection, Region, SchedulingPolicy, ShareMode, TaskInfo};
use pretty_assertions::assert_eq;
use procps_core::{Field, LinuxSizes, Process, RegionTotals, Usage};
use rstest::rstest;

/// `VM_PROT_READ | VM_PROT_EXECUTE`.
const CODE: u32 = 0x5;

/// `VM_PROT_READ | VM_PROT_WRITE`.
const DATA: u32 = 0x3;

/// `VM_PROT_READ`.
const READ_ONLY: u32 = 0x1;

/// A private region of `size` bytes with `protection`, of which
/// `private_resident` bytes are resident and private.
fn region(size: u64, protection: u32, is_submap: bool, private_resident: u64) -> Region {
    shared_region(
        size,
        protection,
        is_submap,
        private_resident,
        ShareMode::Private,
    )
}

/// A region like [`region`], with `share_mode`.
fn shared_region(
    size: u64,
    protection: u32,
    is_submap: bool,
    private_resident: u64,
    share_mode: ShareMode,
) -> Region {
    Region {
        address: 0,
        size,
        protection: Protection::from(protection),
        max_protection: Protection::from(0x7),
        share_mode,
        user_tag: 0,
        resident: private_resident,
        private_resident,
        shared_resident: 0,
        shared_now_private: 0,
        swapped_out: 0,
        dirtied: 0,
        reference_count: 1,
        object_id: None,
        is_submap,
        is_shared: false,
    }
}

/// A task with `virtual_size` and `resident_size` bytes.
const fn task(virtual_size: u64, resident_size: u64) -> TaskInfo {
    TaskInfo {
        virtual_size,
        resident_size,
        user_time: Duration::ZERO,
        system_time: Duration::ZERO,
        policy: SchedulingPolicy::Timeshare,
        faults: 0,
        pageins: 0,
        copy_on_write_faults: 0,
        messages_sent: 0,
        messages_received: 0,
        mach_system_calls: 0,
        unix_system_calls: 0,
        context_switches: 0,
        threads: 1,
        running_threads: 0,
        priority: 31,
    }
}

/// A process with this process's identity, and the task and regions given.
fn process(
    task: Option<Field<TaskInfo>>,
    regions: Option<Field<Vec<Region>>>,
) -> Result<Process, darwin_proc::Error> {
    let info = Pid::current().info()?;

    let process = Process {
        usage: task.map(|task| Usage {
            task,
            resources: Field::Denied,
            counters: Field::Denied,
        }),
        ..Process::from_identity(info, Field::Unsupported, Field::Unsupported)
    };

    Ok(match regions {
        Some(regions) => process.with_regions(regions),
        None => process,
    })
}

#[test]
fn the_regions_and_the_task_give_every_size() -> Result<(), Box<dyn std::error::Error>> {
    let regions = vec![
        region(1000, CODE, false, 600),
        region(5000, CODE, true, 0),
        region(300, DATA, false, 200),
        region(70, READ_ONLY, false, 10),
        shared_region(4000, DATA, false, 0, ShareMode::TrueShared),
    ];
    let process = process(
        Some(Field::Available(task(10_000, 2000))),
        Some(Field::Available(regions)),
    )?;

    assert_eq!(
        process.linux_sizes(),
        LinuxSizes {
            virtual_size: Some(Field::Available(10_000)),
            resident: Some(Field::Available(2000)),
            text: Some(Field::Available(1000)),
            data: Some(Field::Available(9000)),
            data_and_stack: Some(Field::Available(300)),
            unique: Some(Field::Available(810)),
            proportional: Some(Field::Unsupported),
        }
    );

    Ok(())
}

#[test]
fn the_totals_leave_out_submaps_and_shared_regions() {
    let regions = [
        region(1000, CODE, false, 600),
        region(5000, CODE, true, 0),
        region(300, DATA, false, 200),
        region(7000, DATA, true, 0),
        region(70, READ_ONLY, false, 10),
        shared_region(4000, DATA, false, 0, ShareMode::TrueShared),
        shared_region(20, DATA, false, 0, ShareMode::CopyOnWrite),
    ];

    assert_eq!(
        RegionTotals::from(regions.as_slice()),
        RegionTotals {
            executable: 1000,
            private_writable: 320,
        }
    );
}

#[test]
fn the_totals_give_trs_drs_and_size_without_the_regions() -> Result<(), Box<dyn std::error::Error>>
{
    let process = Process {
        region_totals: Some(Field::Available(RegionTotals {
            executable: 1000,
            private_writable: 300,
        })),
        regions: Some(Field::Denied),
        ..process(Some(Field::Available(task(10_000, 2000))), None)?
    };

    assert_eq!(
        process.linux_sizes(),
        LinuxSizes {
            virtual_size: Some(Field::Available(10_000)),
            resident: Some(Field::Available(2000)),
            text: Some(Field::Available(1000)),
            data: Some(Field::Available(9000)),
            data_and_stack: Some(Field::Available(300)),
            unique: Some(Field::Denied),
            proportional: Some(Field::Unsupported),
        }
    );

    Ok(())
}

#[rstest]
#[case::nothing_requested(None, None, LinuxSizes::default())]
#[case::task_only(
    Some(Field::Available(task(10_000, 2000))),
    None,
    LinuxSizes {
        virtual_size: Some(Field::Available(10_000)),
        resident: Some(Field::Available(2000)),
        ..LinuxSizes::default()
    }
)]
#[case::regions_denied(
    Some(Field::Available(task(10_000, 2000))),
    Some(Field::Denied),
    LinuxSizes {
        virtual_size: Some(Field::Available(10_000)),
        resident: Some(Field::Available(2000)),
        text: Some(Field::Denied),
        data: Some(Field::Denied),
        data_and_stack: Some(Field::Denied),
        unique: Some(Field::Denied),
        proportional: Some(Field::Unsupported),
    }
)]
#[case::task_unsupported(
    Some(Field::Unsupported),
    Some(Field::Available(vec![region(1000, CODE, false, 600)])),
    LinuxSizes {
        virtual_size: Some(Field::Unsupported),
        resident: Some(Field::Unsupported),
        text: Some(Field::Available(1000)),
        data: Some(Field::Unsupported),
        data_and_stack: Some(Field::Available(0)),
        unique: Some(Field::Available(600)),
        proportional: Some(Field::Unsupported),
    }
)]
#[case::task_failed(
    Some(Field::Failed),
    Some(Field::Denied),
    LinuxSizes {
        virtual_size: Some(Field::Failed),
        resident: Some(Field::Failed),
        text: Some(Field::Denied),
        data: Some(Field::Failed),
        data_and_stack: Some(Field::Denied),
        unique: Some(Field::Denied),
        proportional: Some(Field::Unsupported),
    }
)]
#[case::denied(
    Some(Field::Denied),
    Some(Field::Denied),
    LinuxSizes {
        virtual_size: Some(Field::Denied),
        resident: Some(Field::Denied),
        text: Some(Field::Denied),
        data: Some(Field::Denied),
        data_and_stack: Some(Field::Denied),
        unique: Some(Field::Denied),
        proportional: Some(Field::Unsupported),
    }
)]
fn a_size_needs_the_groups_that_it_comes_from(
    #[case] task: Option<Field<TaskInfo>>,
    #[case] regions: Option<Field<Vec<Region>>>,
    #[case] expected: LinuxSizes,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(process(task, regions)?.linux_sizes(), expected);

    Ok(())
}

#[rstest]
#[case::short("launchd", "launchd")]
#[case::sixteen_bytes("com.apple.WebKi1", "com.apple.WebKi")]
#[case::multibyte_at_the_limit("abcdefghijklmné", "abcdefghijklmn")]
fn a_command_name_has_at_most_15_bytes(
    #[case] comm: &str,
    #[case] expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut info = Pid::current().info()?;
    info.comm = comm.to_owned();
    let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);

    assert_eq!(process.linux_comm(), expected);

    Ok(())
}
