// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2026 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 library/pids.c and src/ps/display.c.

use std::{cmp::Ordering, os::unix::ffi::OsStrExt};

use procps_core::{FieldGroup, LinuxTask};

use crate::{
    linux,
    ps::{
        format::{Order, SortKey},
        print::{self, Context},
        version_order,
    },
};

/// A value of procps-ng's process library that `ps` sorts by (`PIDS_*`),
/// named after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Item {
    AddrCodeEnd,
    AddrCodeStart,
    AddrCurrEip,
    AddrCurrEsp,
    AddrStackStart,
    AutogrpId,
    AutogrpNice,
    CapsPermitted,
    Cgname,
    Cgroup,
    Cmd,
    Cmdline,
    DockerId,
    Exe,
    Flags,
    FltMaj,
    FltMin,
    IdEgid,
    IdEgroup,
    IdEuid,
    IdEuser,
    IdFgid,
    IdFgroup,
    IdFuid,
    IdFuser,
    IdLogin,
    IdPgrp,
    IdPid,
    IdPpid,
    IdRgid,
    IdRgroup,
    IdRuid,
    IdRuser,
    IdSession,
    IdSgid,
    IdSgroup,
    IdSuid,
    IdSuser,
    IdTgid,
    IdTpgid,
    IoReadBytes,
    IoReadChars,
    IoReadOps,
    IoWriteBytes,
    IoWriteCbytes,
    IoWriteChars,
    IoWriteOps,
    Lxcname,
    MemResPgs,
    MemShrPgs,
    Nice,
    Nlwp,
    Noop,
    NsCgroup,
    NsIpc,
    NsMnt,
    NsNet,
    NsPid,
    NsTime,
    NsUser,
    NsUts,
    OomAdj,
    OomScore,
    OpenFiles,
    Priority,
    PriorityRt,
    Processor,
    ProcessorNode,
    RssRlim,
    SchedClass,
    SchedClassstr,
    SdMach,
    SdOuid,
    SdSeat,
    SdSess,
    SdSlice,
    SdUnit,
    SdUunit,
    Sigblocked,
    Sigcatch,
    Sigignore,
    Signals,
    Sigpending,
    SmapHugeTlbprv,
    SmapPrvTotal,
    SmapPss,
    State,
    Supgids,
    Supgroups,
    TicsAll,
    TicsBegan,
    TicsUser,
    TicsUserC,
    TimeAll,
    TimeElapsed,
    TtyName,
    Utilization,
    UtilizationC,
    VmData,
    VmExe,
    VmLib,
    VmRss,
    VmRssLocked,
    VmSize,
    VmStack,
    VsizeBytes,
    WchanName,
}

/// A value to sort by, with procps-ng's comparison for its type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    /// A value that the source could not read, or that macOS lacks. It
    /// sorts as a value lower than every other value: first in ascending
    /// order and last in descending order.
    Missing,
    /// A signed integer.
    Signed(i64),
    /// An unsigned integer.
    Unsigned(u64),
    /// A real number.
    Real(f64),
    /// Text, which procps-ng compares with `strcoll`. `ps` compares the
    /// bytes, as `strcoll` does in the C locale.
    Text(Vec<u8>),
    /// Text with numbers in it, which procps-ng compares with `strverscmp`.
    Version(Vec<u8>),
    /// A value that procps-ng has no data for, so every process has the same
    /// value.
    Same,
}

impl Value {
    /// The order of `self` and `other`, which come from the same item.
    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Missing, Self::Missing) => Ordering::Equal,
            (Self::Missing, _) => Ordering::Less,
            (_, Self::Missing) => Ordering::Greater,
            (Self::Signed(left), Self::Signed(right)) => left.cmp(right),
            (Self::Unsigned(left), Self::Unsigned(right)) => left.cmp(right),
            (Self::Real(left), Self::Real(right)) => left.total_cmp(right),
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
            (Self::Version(left), Self::Version(right)) => version_order::compare(left, right),
            _ => Ordering::Equal,
        }
    }
}

impl Item {
    /// Whether this project computes the value yet. Task 5.4 implements the
    /// rest.
    pub(crate) const fn is_implemented(self) -> bool {
        !matches!(
            self,
            Self::Exe
                | Self::IoReadBytes
                | Self::IoWriteBytes
                | Self::MemResPgs
                | Self::MemShrPgs
                | Self::OpenFiles
                | Self::RssRlim
                | Self::SmapPrvTotal
                | Self::Supgids
                | Self::Supgroups
                | Self::TicsUserC
                | Self::UtilizationC
                | Self::VmData
                | Self::VmExe
                | Self::VmLib
                | Self::VmRssLocked
                | Self::VmStack
        )
    }

    /// The field groups that the value comes from.
    pub(crate) const fn groups(self) -> &'static [FieldGroup] {
        use FieldGroup::{Arguments, Threads, Usage};

        match self {
            Self::Cmdline => &[Arguments],
            Self::State => &[Threads],
            Self::FltMaj
            | Self::FltMin
            | Self::Nlwp
            | Self::Priority
            | Self::PriorityRt
            | Self::SchedClass
            | Self::SchedClassstr
            | Self::TicsAll
            | Self::TicsUser
            | Self::TimeAll
            | Self::Utilization
            | Self::VmRss
            | Self::VmSize
            | Self::VsizeBytes => &[Usage],
            _ => &[],
        }
    }

    /// The value of `task`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm for each of procps-ng's items keeps the table readable"
    )]
    fn value(self, context: &mut Context<'_>, task: &LinuxTask<'_>) -> Value {
        let process = task.process();
        let info = &process.info;
        let credentials = &info.credentials;
        let unsigned = |value: Option<u64>| value.map_or(Value::Missing, Value::Unsigned);

        match self {
            Self::Cmd => Value::Text(task.command(context.charset()).as_bytes().to_vec()),
            Self::Cmdline => Value::Text(task.command_line(context.charset()).as_bytes().to_vec()),
            Self::Flags => Value::Unsigned(process.linux_flags()),
            Self::FltMaj => unsigned(print::major_faults(process)),
            Self::FltMin => unsigned(print::minor_faults(process)),
            Self::IdEgid | Self::IdFgid => Value::Unsigned(credentials.egid.as_raw().into()),
            Self::IdEuid | Self::IdFuid => Value::Unsigned(credentials.euid.as_raw().into()),
            Self::IdRgid => Value::Unsigned(credentials.rgid.as_raw().into()),
            Self::IdRuid => Value::Unsigned(credentials.ruid.as_raw().into()),
            Self::IdSgid => Value::Unsigned(credentials.svgid.as_raw().into()),
            Self::IdSuid => Value::Unsigned(credentials.svuid.as_raw().into()),
            Self::IdEuser | Self::IdFuser => {
                Value::Text(context.names().user(credentials.euid.as_raw()).to_vec())
            }
            Self::IdRuser => Value::Text(context.names().user(credentials.ruid.as_raw()).to_vec()),
            Self::IdSuser => Value::Text(context.names().user(credentials.svuid.as_raw()).to_vec()),
            Self::IdEgroup | Self::IdFgroup => {
                Value::Text(context.names().group(credentials.egid.as_raw()).to_vec())
            }
            Self::IdRgroup => {
                Value::Text(context.names().group(credentials.rgid.as_raw()).to_vec())
            }
            Self::IdSgroup => {
                Value::Text(context.names().group(credentials.svgid.as_raw()).to_vec())
            }
            Self::IdPgrp => Value::Signed(info.pgid.as_raw().into()),
            Self::IdPid => i64::try_from(task.id()).map_or(Value::Missing, Value::Signed),
            Self::IdPpid => Value::Signed(info.ppid.as_raw().into()),
            Self::IdSession => linux::session(context.identities(), process)
                .map_or(Value::Missing, |session| {
                    Value::Signed(session.as_raw().into())
                }),
            Self::IdTgid => Value::Signed(info.pid.as_raw().into()),
            Self::IdTpgid => Value::Signed(
                info.terminal
                    .map_or(-1, |terminal| terminal.foreground_group.as_raw())
                    .into(),
            ),
            Self::Nice => Value::Signed(info.nice.into()),
            Self::Nlwp => process.thread_count().map_or(Value::Missing, |count| {
                u64::try_from(count).map_or(Value::Missing, Value::Unsigned)
            }),
            Self::Priority => Value::Signed(process.linux_priority().priority.into()),
            Self::PriorityRt => process
                .linux_priority()
                .rt_priority
                .map_or(Value::Missing, |priority| Value::Signed(priority.into())),
            Self::SchedClass => {
                Value::Signed(print::class_number(&process.linux_priority()).into())
            }
            Self::SchedClassstr => Value::Text(process.linux_priority().class().into()),
            Self::Sigcatch => Value::Text(print::signal_mask(info.caught_signals.bits())),
            Self::Sigignore => Value::Text(print::signal_mask(info.ignored_signals.bits())),
            Self::State => task.state_letter().map_or(Value::Missing, |letter| {
                Value::Signed(u32::from(letter).into())
            }),
            Self::TicsAll => unsigned(print::cpu_ticks(process)),
            Self::TicsBegan => Value::Unsigned(context.clock().tick(info.start_time)),
            Self::TicsUser => unsigned(print::user_ticks(process)),
            Self::TimeAll => print::cpu_seconds(process).map_or(Value::Missing, Value::Real),
            Self::TimeElapsed => Value::Real(print::elapsed(context, process)),
            Self::TtyName => Value::Version(print::terminal(process, false)),
            Self::Utilization => {
                print::utilization(context, process).map_or(Value::Missing, Value::Real)
            }
            Self::VmRss => unsigned(print::sizes(process).map(|(_, resident)| resident / 1024)),
            Self::VmSize => unsigned(print::sizes(process).map(|(size, _)| size / 1024)),
            Self::VsizeBytes => unsigned(print::sizes(process).map(|(size, _)| size)),
            // Linux concepts that macOS lacks, and values that procps-ng
            // cannot print either.
            Self::AddrCodeEnd
            | Self::AddrCodeStart
            | Self::AddrCurrEip
            | Self::AddrCurrEsp
            | Self::AddrStackStart
            | Self::AutogrpId
            | Self::AutogrpNice
            | Self::CapsPermitted
            | Self::Cgname
            | Self::Cgroup
            | Self::DockerId
            | Self::IdLogin
            | Self::IoReadChars
            | Self::IoReadOps
            | Self::IoWriteCbytes
            | Self::IoWriteChars
            | Self::IoWriteOps
            | Self::Lxcname
            | Self::NsCgroup
            | Self::NsIpc
            | Self::NsMnt
            | Self::NsNet
            | Self::NsPid
            | Self::NsTime
            | Self::NsUser
            | Self::NsUts
            | Self::OomAdj
            | Self::OomScore
            | Self::Processor
            | Self::ProcessorNode
            | Self::SdMach
            | Self::SdOuid
            | Self::SdSeat
            | Self::SdSess
            | Self::SdSlice
            | Self::SdUnit
            | Self::SdUunit
            | Self::Sigblocked
            | Self::Signals
            | Self::Sigpending
            | Self::SmapHugeTlbprv
            | Self::SmapPss
            | Self::WchanName
            | Self::Noop => Value::Same,
            Self::Exe
            | Self::IoReadBytes
            | Self::IoWriteBytes
            | Self::MemResPgs
            | Self::MemShrPgs
            | Self::OpenFiles
            | Self::RssRlim
            | Self::SmapPrvTotal
            | Self::Supgids
            | Self::Supgroups
            | Self::TicsUserC
            | Self::UtilizationC
            | Self::VmData
            | Self::VmExe
            | Self::VmLib
            | Self::VmRssLocked
            | Self::VmStack => unreachable!("ps refuses sort keys that it does not implement"),
        }
    }
}

/// Sorts `tasks` by each of `keys` in turn, as procps-ng's `fancy_spew`
/// does. Each sort is stable, so the last key is the most significant, and
/// tasks that every key finds equal keep their order. A key whose order is
/// [`Order::Ignored`] does not sort.
pub(crate) fn sort(tasks: &mut Vec<LinuxTask<'_>>, keys: &[SortKey], context: &mut Context<'_>) {
    for key in keys {
        let descending = match key.order {
            Order::Ascending => false,
            Order::Descending => true,
            Order::Ignored => continue,
        };
        let item = key.specifier.sort;
        let mut valued: Vec<(Value, LinuxTask<'_>)> = tasks
            .drain(..)
            .map(|task| (item.value(context, &task), task))
            .collect();

        valued.sort_by(|(left, _), (right, _)| {
            let order = left.compare(right);

            if descending { order.reverse() } else { order }
        });
        tasks.extend(valued.into_iter().map(|(_, task)| task));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cmp::Ordering,
        ffi::OsString,
        time::{Duration, SystemTime},
    };

    use darwin_proc::{Pid, ResourceUsage, RunState, Status, TaskInfo, Terminal};
    use pretty_assertions::assert_eq;
    use procps_core::{
        Field, Process, Usage,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{Value, sort};
    use crate::ps::{
        format::{Order, SortKey},
        names::Names,
        options::Modifiers,
        print::{
            Context,
            tests::{TAKEN, resources, settings, snapshot, system, task},
        },
        specifier,
    };

    fn key(name: &str, order: Order) -> SortKey {
        SortKey {
            specifier: specifier::find(name.as_bytes()).expect("a specifier"),
            order,
        }
    }

    #[rstest]
    #[case::one_key(vec![key("comm", Order::Ascending)], vec![30, 10, 20, 40])]
    #[case::descending(vec![key("comm", Order::Descending)], vec![40, 10, 20, 30])]
    #[case::last_key_most_significant(
        vec![key("pid", Order::Descending), key("comm", Order::Ascending)],
        vec![30, 20, 10, 40]
    )]
    #[case::ignored_key(
        vec![key("comm", Order::Ascending), key("pid", Order::Ignored)],
        vec![30, 10, 20, 40]
    )]
    #[case::equal_values_keep_their_order(vec![key("ni", Order::Descending)], vec![10, 20, 30, 40])]
    fn each_key_sorts_the_tasks_again(#[case] keys: Vec<SortKey>, #[case] expected: Vec<i32>) {
        let system = system();
        let snapshot = snapshot(vec![
            process(10, "cat"),
            process(20, "cat"),
            process(30, "bash"),
            process(40, "zsh"),
        ]);
        let mut context = Context::new(
            &snapshot,
            &snapshot,
            settings(Modifiers::empty()),
            Names::new(&system),
        );
        let mut tasks: Vec<_> = snapshot
            .processes
            .iter()
            .map(|process| process.linux_task())
            .collect();

        sort(&mut tasks, &keys, &mut context);

        let pids: Vec<i32> = tasks
            .iter()
            .map(|task| task.process().info.pid.as_raw())
            .collect();

        assert_eq!(pids, expected);
    }

    /// A process with `pid` and `comm` that started `age` seconds before the
    /// snapshot and has used `cpu` of CPU time, with one thread in
    /// `state`, the command line `arguments`, and the terminal's foreground
    /// process group `foreground`, if it has a terminal.
    fn fixture(
        pid: i32,
        comm: &str,
        age: u64,
        cpu: Duration,
        state: RunState,
        arguments: &str,
        foreground: Option<i32>,
    ) -> Process {
        let mut process = process(pid, comm);

        process.info.start_time =
            SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN) - Duration::from_secs(age);
        process.info.terminal = foreground.map(|group| Terminal {
            device: 0,
            foreground_group: Pid::from(group),
        });
        process.session = Field::Available(Pid::from(pid));
        process.arguments = Some(Field::Available(vec![OsString::from(arguments)]));
        process.threads = Some(Field::Available(vec![thread(1, b"", state)]));
        process.usage = Some(Usage {
            task: Field::Available(TaskInfo {
                user_time: cpu,
                system_time: Duration::ZERO,
                ..task()
            }),
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });

        process
    }

    /// Three processes whose values put them in a different order for each
    /// key. 30 is a zombie in 20's process group. The source could not read
    /// its session, its arguments or its task information.
    fn three() -> Vec<Process> {
        let mut zombie = fixture(
            30,
            "ccc",
            200,
            Duration::from_secs(2),
            RunState::Waiting,
            "",
            Some(3),
        );
        zombie.info.status = Status::Zombie;
        zombie.info.pgid = Pid::from(20);
        zombie.session = Field::Failed;
        zombie.arguments = Some(Field::Unsupported);
        zombie.usage = Some(Usage {
            task: Field::Failed,
            resources: Field::Available(ResourceUsage {
                user_time: Duration::from_secs(2),
                system_time: Duration::ZERO,
                ..resources()
            }),
            counters: Field::Unsupported,
        });

        vec![
            fixture(
                10,
                "aaa",
                300,
                Duration::from_secs(1),
                RunState::Waiting,
                "zeta",
                None,
            ),
            fixture(
                20,
                "bbb",
                100,
                Duration::from_secs(3),
                RunState::Running,
                "alpha",
                Some(0),
            ),
            zombie,
        ]
    }

    /// The pids of `processes` sorted by `key`.
    fn sorted(processes: Vec<Process>, key: SortKey) -> Vec<i32> {
        let system = system();
        let snapshot = snapshot(processes);
        let mut context = Context::new(
            &snapshot,
            &snapshot,
            settings(Modifiers::empty()),
            Names::new(&system),
        );
        let mut tasks: Vec<_> = snapshot
            .processes
            .iter()
            .map(|process| process.linux_task())
            .collect();

        sort(&mut tasks, &[key], &mut context);

        tasks
            .iter()
            .map(|task| task.process().info.pid.as_raw())
            .collect()
    }

    #[rstest]
    #[case::start_time(key("start_time", Order::Ascending), vec![10, 30, 20])]
    #[case::lstart(key("lstart", Order::Descending), vec![20, 30, 10])]
    #[case::elapsed(key("etime", Order::Ascending), vec![20, 30, 10])]
    #[case::cpu_use(key("%cpu", Order::Ascending), vec![10, 30, 20])]
    #[case::cpu_time(key("time", Order::Descending), vec![20, 30, 10])]
    #[case::state(key("stat", Order::Ascending), vec![20, 10, 30])]
    #[case::session(key("sid", Order::Descending), vec![20, 30, 10])]
    #[case::command_line(key("args", Order::Ascending), vec![30, 20, 10])]
    #[case::command_name(key("comm", Order::Descending), vec![30, 20, 10])]
    #[case::foreground_group(key("tpgid", Order::Descending), vec![30, 20, 10])]
    fn each_item_sorts_by_procps_ng_s_value(#[case] key: SortKey, #[case] expected: Vec<i32>) {
        assert_eq!(sorted(three(), key), expected);
    }

    // procps-ng multiplies the ticks by 100 as a `float`, which rounds
    // 16777217 ticks to 16777216, so the two processes use the CPU equally.
    #[test]
    fn cpu_use_is_computed_with_procps_ng_s_float() {
        let busy = |pid, cpu| fixture(pid, "busy", 100, cpu, RunState::Running, "busy", None);
        let processes = vec![
            busy(10, Duration::from_millis(167_772_160)),
            busy(20, Duration::from_millis(167_772_170)),
        ];

        assert_eq!(sorted(processes, key("%cpu", Order::Descending)), [10, 20]);
    }

    #[test]
    fn many_equal_values_keep_their_order() {
        let processes: Vec<Process> = (1..=60)
            .map(|pid| {
                let mut process = process(pid, "same");
                process.info.nice = i8::try_from(pid % 3).expect("a nice value");
                process
            })
            .collect();
        // Sorted by nice value, and within each value by descending pid, as
        // the first key left them.
        let expected: Vec<i32> = (0..3)
            .flat_map(|nice| (1..=60).rev().filter(move |pid| pid % 3 == nice))
            .collect();

        let system = system();
        let snapshot = snapshot(processes);
        let mut context = Context::new(
            &snapshot,
            &snapshot,
            settings(Modifiers::empty()),
            Names::new(&system),
        );
        let mut tasks: Vec<_> = snapshot
            .processes
            .iter()
            .map(|process| process.linux_task())
            .collect();

        sort(
            &mut tasks,
            &[key("pid", Order::Descending), key("ni", Order::Ascending)],
            &mut context,
        );

        let pids: Vec<i32> = tasks
            .iter()
            .map(|task| task.process().info.pid.as_raw())
            .collect();

        assert_eq!(pids, expected);
    }

    #[rstest]
    #[case::missing_first(Value::Missing, Value::Signed(-5), Ordering::Less)]
    #[case::both_missing(Value::Missing, Value::Missing, Ordering::Equal)]
    #[case::signed(Value::Signed(-1), Value::Signed(1), Ordering::Less)]
    #[case::unsigned(Value::Unsigned(10), Value::Unsigned(9), Ordering::Greater)]
    #[case::real(Value::Real(0.5), Value::Real(0.25), Ordering::Greater)]
    #[case::text(Value::Text(b"B".to_vec()), Value::Text(b"a".to_vec()), Ordering::Less)]
    #[case::version(
        Value::Version(b"ttys2".to_vec()),
        Value::Version(b"ttys10".to_vec()),
        Ordering::Less
    )]
    #[case::same(Value::Same, Value::Same, Ordering::Equal)]
    fn values_compare_as_procps_ng_compares_them(
        #[case] left: Value,
        #[case] right: Value,
        #[case] expected: Ordering,
    ) {
        assert_eq!(left.compare(&right), expected);
    }
}
