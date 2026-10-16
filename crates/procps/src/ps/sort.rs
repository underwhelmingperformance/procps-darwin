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
/// named after it, or the value of a Darwin column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Item {
    AddrCodeEnd,
    AddrCodeStart,
    AddrCurrEip,
    AddrCurrEsp,
    AddrStackStart,
    Architecture,
    AutogrpId,
    AutogrpNice,
    CapsPermitted,
    Cgname,
    Cgroup,
    Cmd,
    Cmdline,
    ContextSwitches,
    Cycles,
    DockerId,
    Energy,
    Exe,
    Flags,
    FltMaj,
    FltMin,
    Footprint,
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
    Instructions,
    IoReadBytes,
    IoReadChars,
    IoReadOps,
    IoWriteBytes,
    IoWriteCbytes,
    IoWriteChars,
    IoWriteOps,
    Lxcname,
    MaxFootprint,
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
    Wired,
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
    /// The field groups that the value comes from.
    pub(crate) const fn groups(self) -> &'static [FieldGroup] {
        use FieldGroup::{FileDescriptors, Regions, Threads, Usage};

        match self {
            Self::Cmdline => &[FieldGroup::Arguments],
            Self::State => &[Threads],
            Self::OpenFiles => &[FileDescriptors],
            Self::SmapPrvTotal | Self::VmExe => &[Usage, Regions],
            Self::FltMaj
            | Self::FltMin
            | Self::IoReadBytes
            | Self::IoWriteBytes
            | Self::MemResPgs
            | Self::Nlwp
            | Self::Priority
            | Self::PriorityRt
            | Self::SchedClass
            | Self::SchedClassstr
            | Self::TicsAll
            | Self::TicsUser
            | Self::TicsUserC
            | Self::TimeAll
            | Self::Utilization
            | Self::UtilizationC
            | Self::VmRss
            | Self::VmRssLocked
            | Self::VmSize
            | Self::VsizeBytes
            | Self::ContextSwitches
            | Self::Cycles
            | Self::Energy
            | Self::Footprint
            | Self::Instructions
            | Self::MaxFootprint
            | Self::Wired => &[Usage],
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
            Self::Exe => Value::Text(print::executable(context, process)),
            Self::AddrCurrEip | Self::AddrCurrEsp => Value::Unsigned(0),
            // Linux's start of a stack that `ps` may read is an address above
            // 0, so the unknown starts sort after the zeros, as on Linux.
            Self::AddrStackStart => {
                Value::Unsigned(print::stack_start(context, process).unwrap_or(u64::MAX))
            }
            Self::Architecture => Value::Text(print::architecture(process).to_vec()),
            Self::ContextSwitches => {
                unsigned(print::task_info(process).map(|task| task.context_switches.into()))
            }
            Self::Cycles => unsigned(print::counters(process).map(|counters| counters.cycles)),
            Self::Energy => unsigned(print::energy_millijoules(process)),
            Self::Footprint => unsigned(
                print::resources(process).map(|resources| resources.physical_footprint >> 10),
            ),
            Self::Instructions => {
                unsigned(print::counters(process).map(|counters| counters.instructions))
            }
            Self::MaxFootprint => unsigned(
                print::resources(process).map(|resources| resources.peak_physical_footprint >> 10),
            ),
            Self::Wired => {
                unsigned(print::resources(process).map(|resources| resources.wired_size >> 10))
            }
            Self::IoReadBytes => {
                unsigned(print::counters(process).map(|counters| counters.disk_bytes_read))
            }
            Self::IoWriteBytes => {
                unsigned(print::counters(process).map(|counters| counters.disk_bytes_written))
            }
            Self::MemResPgs => {
                let page_size = context.page_size().max(1);

                unsigned(print::sizes(process).map(|(_, resident)| resident / page_size))
            }
            Self::OpenFiles => print::open_files(process)
                .and_then(|count| i64::try_from(count).ok())
                .map_or(Value::Missing, Value::Signed),
            Self::SmapPrvTotal => unsigned(print::unique_size(process)),
            Self::Supgids => Value::Text(print::supplementary_gids(process)),
            Self::Supgroups => Value::Text(print::supplementary_groups(context, process)),
            Self::TicsUserC => unsigned(print::user_ticks_with_children(process)),
            Self::UtilizationC => print::utilization_with_children(context, process)
                .map_or(Value::Missing, Value::Real),
            // Linux shows `VmExe` in `status` to every user, so this key does
            // not apply the access check that hides the code size in `trs`.
            Self::VmExe => unsigned(
                process
                    .linux_sizes()
                    .text
                    .and_then(|size| size.available().copied())
                    .map(|size| size >> 10),
            ),
            Self::VmRssLocked => unsigned(
                process
                    .usage
                    .as_ref()
                    .and_then(|usage| usage.resources.available())
                    .map(|resources| resources.wired_size >> 10),
            ),
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
            // cannot print either. macOS has a limit on the resident size,
            // but no call reads it for another process. `ps` does not
            // approximate the shared, data, stack and library sizes, which
            // only procps-ng's placeholders sort by.
            Self::AddrCodeEnd
            | Self::AddrCodeStart
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
            | Self::MemShrPgs
            | Self::RssRlim
            | Self::VmData
            | Self::VmLib
            | Self::VmStack
            | Self::Noop => Value::Same,
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
        path::PathBuf,
        time::{Duration, SystemTime},
    };

    use darwin_proc::{
        Gid, Pid, ProcessFlags, Protection, Region, ResourceCounters, ResourceUsage, RunState,
        ShareMode, Status, TaskInfo, Terminal, Uid,
    };
    use pretty_assertions::assert_eq;
    use procps_core::{
        Field, Process, RegionTotals, Usage,
        helper::Caller,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{Value, sort};
    use crate::ps::{
        format::{Order, SortKey},
        names::Names,
        options::Modifiers,
        print::{
            Context, Settings,
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
        sorted_for(processes, key, settings(Modifiers::empty()))
    }

    /// The pids of `processes` sorted by `key`, for the caller and the other
    /// settings in `settings`.
    fn sorted_for(processes: Vec<Process>, key: SortKey, settings: Settings) -> Vec<i32> {
        let system = system();
        let snapshot = snapshot(processes);
        let mut context = Context::new(&snapshot, &snapshot, settings, Names::new(&system));
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

    /// The values of Linux's private files and of `statm` that a process
    /// sorts by.
    #[derive(Clone, Copy)]
    struct Private {
        executable: &'static str,
        group: u32,
        descriptors: i32,
        read: u64,
        written: u64,
        unique: u64,
        children: Duration,
        children_system: Duration,
        resident: u64,
        code: u64,
        wired: u64,
    }

    /// A process with `pid` and the values `values`.
    fn private(pid: i32, values: Private) -> Process {
        let mut process = fixture(
            pid,
            "same",
            100,
            Duration::ZERO,
            RunState::Waiting,
            "a",
            None,
        );

        process.executable = Field::Available(PathBuf::from(values.executable));
        process.info.credentials.groups = vec![Gid::from(values.group)];
        process =
            process.with_file_descriptors(Field::Available((0..values.descriptors).collect()));
        process.usage = Some(Usage {
            task: Field::Available(TaskInfo {
                resident_size: values.resident,
                user_time: Duration::ZERO,
                system_time: Duration::ZERO,
                ..task()
            }),
            resources: Field::Available(ResourceUsage {
                children_user_time: values.children,
                children_system_time: values.children_system,
                wired_size: values.wired,
                ..resources()
            }),
            counters: Field::Available(ResourceCounters {
                disk_bytes_read: values.read,
                disk_bytes_written: values.written,
                logical_writes: 0,
                instructions: 0,
                cycles: 0,
                energy_nanojoules: 0,
            }),
        });
        process.region_totals = Some(Field::Available(RegionTotals {
            executable: values.code,
            private_writable: 0,
        }));
        process.regions = Some(Field::Available(vec![Region {
            address: 0,
            size: values.unique,
            protection: Protection::from(0x3),
            max_protection: Protection::from(0x7),
            share_mode: ShareMode::Private,
            user_tag: 0,
            resident: values.unique,
            private_resident: values.unique,
            shared_resident: 0,
            shared_now_private: 0,
            swapped_out: 0,
            dirtied: 0,
            reference_count: 1,
            object_id: None,
            is_submap: false,
            is_shared: false,
        }]));

        process
    }

    /// Three processes whose private values put them in different orders.
    fn three_private() -> Vec<Process> {
        vec![
            private(
                10,
                Private {
                    executable: "/b",
                    group: 20,
                    descriptors: 3,
                    read: 200,
                    written: 100,
                    unique: 64 << 10,
                    children: Duration::from_secs(1),
                    children_system: Duration::from_secs(5),
                    resident: 32 << 20,
                    code: 128 << 10,
                    wired: 4 << 10,
                },
            ),
            private(
                20,
                Private {
                    executable: "/c",
                    group: 0,
                    descriptors: 5,
                    read: 300,
                    written: 50,
                    unique: 128 << 10,
                    children: Duration::from_secs(3),
                    children_system: Duration::ZERO,
                    resident: 16 << 20,
                    code: 64 << 10,
                    wired: 8 << 10,
                },
            ),
            private(
                30,
                Private {
                    executable: "/a",
                    group: 12,
                    descriptors: 1,
                    read: 100,
                    written: 200,
                    unique: 32 << 10,
                    children: Duration::from_secs(2),
                    children_system: Duration::ZERO,
                    resident: 64 << 20,
                    code: 256 << 10,
                    wired: 2 << 10,
                },
            ),
        ]
    }

    // Linux reports the start of the stack as 0 to a caller that may not read
    // the process, and as a non-zero address for a readable process with
    // memory, so procps-ng sorts the zeros first. `ps` does not know the start
    // for a readable process, and sorts those processes after the zeros, equal
    // to each other.
    #[rstest]
    #[case::ascending(Order::Ascending, [10, 20, 30])]
    #[case::descending(Order::Descending, [20, 30, 10])]
    fn the_start_of_the_stack_sorts_as_procps_ng_sorts_it(
        #[case] order: Order,
        #[case] expected: [i32; 3],
    ) {
        let mut stranger = process(10, "other");
        stranger.info.credentials.euid = Uid::from(502);
        let settings = Settings {
            caller: Caller::new(Uid::from(501), Gid::from(20)),
            ..settings(Modifiers::empty())
        };

        assert_eq!(
            sorted_for(
                vec![process(20, "own"), stranger, process(30, "own")],
                key("stackp", order),
                settings
            ),
            expected
        );
    }

    // `exe` shows `-` for every process here, so sorting by the hidden paths
    // would reveal their order.
    #[test]
    fn the_executable_sorts_as_ps_shows_it() {
        let settings = Settings {
            caller: Caller::new(Uid::from(502), Gid::from(20)),
            ..settings(Modifiers::empty())
        };

        assert_eq!(
            sorted_for(three_private(), key("exe", Order::Ascending), settings),
            [10, 20, 30]
        );
    }

    // The placeholders `vm_data`, `vm_stack`, `vm_lib` and `m_share`, and
    // `lim`, sort by values that `ps` does not have, so every process has the
    // same value.
    #[rstest]
    #[case::executable("exe", vec![30, 10, 20])]
    #[case::group_ids("supgid", vec![20, 30, 10])]
    #[case::group_names("supgrp", vec![30, 10, 20])]
    #[case::descriptors("fds", vec![30, 10, 20])]
    #[case::read("rbytes", vec![30, 10, 20])]
    #[case::written("wbytes", vec![20, 10, 30])]
    #[case::unique("uss", vec![30, 10, 20])]
    #[case::children_user_time("cutime", vec![10, 30, 20])]
    #[case::cpu_use_with_children("cuc", vec![30, 20, 10])]
    #[case::resident_pages("resident", vec![20, 10, 30])]
    #[case::code("vm_exe", vec![20, 10, 30])]
    #[case::locked("vm_lock", vec![30, 10, 20])]
    #[case::limit("lim", vec![10, 20, 30])]
    #[case::data("vm_data", vec![10, 20, 30])]
    #[case::stack("vm_stack", vec![10, 20, 30])]
    #[case::libraries("vm_lib", vec![10, 20, 30])]
    #[case::shared("m_share", vec![10, 20, 30])]
    fn each_private_item_sorts_by_its_value(#[case] name: &str, #[case] expected: Vec<i32>) {
        assert_eq!(
            sorted(three_private(), key(name, Order::Ascending)),
            expected
        );
    }

    /// A process with `pid` whose footprints, wired size, context switches,
    /// and instruction, cycle and energy counters grow with `scale`. The
    /// process runs under Rosetta if `translated` is true.
    fn darwin(pid: i32, scale: u64, translated: bool) -> Process {
        let mut process = fixture(
            pid,
            "same",
            100,
            Duration::ZERO,
            RunState::Waiting,
            "a",
            None,
        );

        if translated {
            process.info.flags = ProcessFlags::from(0x2_0000);
        }

        process.usage = Some(Usage {
            task: Field::Available(TaskInfo {
                context_switches: u32::try_from(scale).expect("a small scale"),
                ..task()
            }),
            resources: Field::Available(ResourceUsage {
                physical_footprint: scale << 20,
                peak_physical_footprint: scale << 21,
                wired_size: scale << 10,
                ..resources()
            }),
            counters: Field::Available(ResourceCounters {
                disk_bytes_read: 0,
                disk_bytes_written: 0,
                logical_writes: 0,
                instructions: scale * 1_000,
                cycles: scale * 2_000,
                energy_nanojoules: scale * 1_000_000,
            }),
        });

        process
    }

    #[rstest]
    #[case::footprint("footprint")]
    #[case::peak_footprint("maxfootprint")]
    #[case::wired("wired")]
    #[case::context_switches("csw")]
    #[case::instructions("instrs")]
    #[case::cycles("cycles")]
    #[case::energy("energy")]
    fn a_darwin_column_sorts_by_its_value(#[case] name: &str) {
        let processes = vec![
            darwin(10, 2, false),
            darwin(20, 3, false),
            darwin(30, 1, false),
        ];

        assert_eq!(sorted(processes, key(name, Order::Ascending)), [30, 10, 20]);
    }

    #[test]
    fn the_architecture_sorts_by_its_name() {
        let processes = vec![darwin(10, 1, true), darwin(20, 1, false)];

        assert_eq!(sorted(processes, key("arch", Order::Ascending)), [20, 10]);
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
