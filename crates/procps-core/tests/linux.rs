// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Maps Darwin process data onto procps-ng's Linux values.

use std::time::Duration;

use darwin_proc::{
    AccountingFlags, Pid, ProcessFlags, RunState, SchedulingPolicy, Status, TaskInfo, Terminal,
    ThreadInfo,
};
use pretty_assertions::assert_eq;
use procps_core::{Field, LinuxPolicy, LinuxPriority, Process, Usage};
use rstest::rstest;

/// `P_TRACED` from `<sys/proc.h>`.
const TRACED: u32 = 0x800;

/// `AFORK` from `<sys/acct.h>`.
const FORKED: u16 = 0x1;

/// `ASU` from `<sys/acct.h>`.
const SUPERUSER: u16 = 0x2;

/// A task with `threads` threads, of which `running` are running.
fn task(policy: SchedulingPolicy, priority: i32, threads: u32, running: u32) -> TaskInfo {
    TaskInfo {
        virtual_size: 0,
        resident_size: 0,
        user_time: Duration::ZERO,
        system_time: Duration::ZERO,
        policy,
        faults: 0,
        pageins: 0,
        copy_on_write_faults: 0,
        messages_sent: 0,
        messages_received: 0,
        mach_system_calls: 0,
        unix_system_calls: 0,
        context_switches: 0,
        threads,
        running_threads: running,
        priority,
    }
}

/// A thread in `run_state`.
fn thread(run_state: RunState) -> ThreadInfo {
    ThreadInfo {
        id: 1,
        user_time: Duration::ZERO,
        system_time: Duration::ZERO,
        cpu_usage: 0,
        policy: SchedulingPolicy::Timeshare,
        run_state,
        swapped: false,
        idle: false,
        current_priority: 31,
        base_priority: 31,
        max_priority: 63,
        name: String::new(),
    }
}

/// What a test varies about a process.
#[derive(Clone, Debug, Default)]
struct Spec {
    status: Option<Status>,
    flags: u32,
    accounting: u16,
    nice: i8,
    session_leader: bool,
    foreground: bool,
    task: Option<TaskInfo>,
    threads: Option<Vec<RunState>>,
}

/// A process with this process's identity, changed as `spec` says.
fn process(spec: Spec) -> Result<Process, darwin_proc::Error> {
    let mut info = Pid::current().info()?;
    info.status = spec.status.unwrap_or(Status::Running);
    info.flags = ProcessFlags::from(spec.flags);
    info.accounting = AccountingFlags::from(spec.accounting);
    info.nice = spec.nice;
    info.session_leader = spec.session_leader;
    info.terminal = Some(Terminal {
        device: 0x1000_0000,
        foreground_group: if spec.foreground {
            info.pgid
        } else {
            Pid::from(1)
        },
    });

    Ok(Process {
        usage: spec.task.map(|task| Usage {
            task: Field::Available(task),
            resources: Field::Denied,
        }),
        threads: spec
            .threads
            .map(|states| Field::Available(states.into_iter().map(thread).collect())),
        ..Process::from_identity(info, Field::Unsupported, Field::Unsupported)
    })
}

#[rstest]
#[case::zombie(Spec { status: Some(Status::Zombie), ..Spec::default() }, "Z")]
#[case::stopped(Spec { status: Some(Status::Stopped), ..Spec::default() }, "T")]
#[case::traced(Spec { status: Some(Status::Stopped), flags: TRACED, ..Spec::default() }, "t")]
#[case::running_thread(
    Spec { threads: Some(vec![RunState::Waiting, RunState::Running]), ..Spec::default() },
    "Rl"
)]
#[case::uninterruptible_thread(
    Spec { threads: Some(vec![RunState::Waiting, RunState::Uninterruptible]), ..Spec::default() },
    "Dl"
)]
#[case::waiting_threads(Spec { threads: Some(vec![RunState::Waiting]), ..Spec::default() }, "S")]
#[case::running_task(
    Spec { task: Some(task(SchedulingPolicy::Timeshare, 31, 1, 1)), ..Spec::default() },
    "R"
)]
#[case::waiting_task(
    Spec { task: Some(task(SchedulingPolicy::Timeshare, 31, 1, 0)), ..Spec::default() },
    "S"
)]
#[case::unknown(Spec::default(), "-")]
#[case::every_modifier(
    Spec {
        nice: -5,
        session_leader: true,
        foreground: true,
        task: Some(task(SchedulingPolicy::Timeshare, 31, 2, 0)),
        ..Spec::default()
    },
    "S<sl+"
)]
#[case::niced(Spec { nice: 10, ..Spec::default() }, "-N")]
fn a_state_code_follows_procps_ng(
    #[case] spec: Spec,
    #[case] expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(process(spec)?.linux_state().to_string(), expected);

    Ok(())
}

#[rstest]
#[case::timeshare(
    Spec { nice: 5, task: Some(task(SchedulingPolicy::Timeshare, 31, 1, 0)), ..Spec::default() },
    LinuxPriority { policy: Some(LinuxPolicy::Other), priority: 25, nice: 5, rt_priority: Some(0) }
)]
#[case::round_robin(
    Spec { task: Some(task(SchedulingPolicy::RoundRobin, 47, 1, 0)), ..Spec::default() },
    LinuxPriority { policy: Some(LinuxPolicy::RoundRobin), priority: -48, nice: 0, rt_priority: Some(47) }
)]
#[case::fifo_above_the_linux_range(
    Spec { task: Some(task(SchedulingPolicy::Fifo, 120, 1, 0)), ..Spec::default() },
    LinuxPriority { policy: Some(LinuxPolicy::Fifo), priority: -100, nice: 0, rt_priority: Some(99) }
)]
#[case::unknown(
    Spec { nice: -3, ..Spec::default() },
    LinuxPriority { policy: None, priority: 17, nice: -3, rt_priority: None }
)]
#[case::unknown_darwin_policy(
    Spec { task: Some(task(SchedulingPolicy::Other(3), 31, 1, 0)), ..Spec::default() },
    LinuxPriority { policy: None, priority: 20, nice: 0, rt_priority: None }
)]
fn a_priority_uses_linux_scales(
    #[case] spec: Spec,
    #[case] expected: LinuxPriority,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(process(spec)?.linux_priority(), expected);

    Ok(())
}

#[rstest]
#[case::other(Some(LinuxPolicy::Other), "TS")]
#[case::fifo(Some(LinuxPolicy::Fifo), "FF")]
#[case::round_robin(Some(LinuxPolicy::RoundRobin), "RR")]
#[case::unknown(None, "-")]
fn a_policy_has_its_procps_ng_class_name(
    #[case] policy: Option<LinuxPolicy>,
    #[case] expected: &str,
) {
    let priority = LinuxPriority {
        policy,
        priority: 20,
        nice: 0,
        rt_priority: None,
    };

    assert_eq!(priority.class(), expected);
}

#[rstest]
#[case::neither(0, 0)]
#[case::forked_only(FORKED, 0x40)]
#[case::superuser(SUPERUSER, 0x100)]
#[case::both(FORKED | SUPERUSER, 0x140)]
fn the_flags_follow_the_accounting_flags(
    #[case] accounting: u16,
    #[case] expected: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let process = process(Spec {
        accounting,
        ..Spec::default()
    })?;

    assert_eq!(process.linux_flags(), expected);

    Ok(())
}
