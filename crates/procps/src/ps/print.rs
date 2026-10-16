// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2026 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2011 Lukas Nykryn <lnykryn@redhat.com>
// SPDX-FileCopyrightText: 1998-2009 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/output.c, and from the parts of
// library/pids.c and library/devname.c that compute the values that it
// prints.

use std::{
    ffi::{CStr, CString},
    os::unix::ffi::OsStrExt,
    time::Duration,
};

use darwin_proc::{CalendarTime, ResourceCounters, ResourceUsage, Status, TaskInfo};
use procps_core::{
    Charset, Field, FieldGroup, LinuxPolicy, LinuxPriority, LinuxTask, Process, Snapshot,
    helper::Caller,
};

use crate::{
    linux::{self, Clock},
    ps::{
        escape,
        forest::{self, Branch},
        names::Names,
        options::{Forest, Modifiers},
    },
};

/// The size of procps-ng's output buffer, which limits the text of a column.
pub(crate) const OUTBUF_SIZE: usize = 2 * 64 * 1024;

/// The size of the buffer that procps-ng formats most values in.
const COLWID: usize = 240;

/// The format of the `lstart` column without `-D`.
const DEFAULT_LSTART_FORMAT: &CStr = c"%a %b %e %H:%M:%S %Y";

/// The function that procps-ng calls to print a column's value (`pr_*`),
/// named after it, or the printer of a Darwin column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Printer {
    Agid,
    Agnice,
    Architecture,
    Args,
    Bsdstart,
    Bsdtime,
    C,
    Cgname,
    Cgroup,
    Cgroupns,
    Class,
    Comm,
    Context,
    ContextSwitches,
    Cp,
    Cycles,
    Docker,
    Drs,
    Dsiz,
    Egid,
    Egroup,
    Eip,
    Energy,
    Environ,
    Esp,
    Etime,
    Etimes,
    Euid,
    Euser,
    Exe,
    Fds,
    Fgid,
    Fgroup,
    Flag,
    Fname,
    Footprint,
    Fuid,
    Fuser,
    Hugetblprv,
    Hugetblshr,
    Instructions,
    Ipcns,
    Lim,
    Lstart,
    Luid,
    Lxcname,
    Majflt,
    MaxFootprint,
    Minflt,
    Mntns,
    Netns,
    Nice,
    Nlwp,
    Nop,
    Numa,
    Oom,
    OomAdj,
    Opri,
    Pcap,
    Pcaps,
    Pcpu,
    Pgid,
    Pidns,
    Pmem,
    Ppid,
    Pri,
    PriApi,
    PriBar,
    PriBaz,
    PriFoo,
    Priority,
    Procs,
    Psr,
    Pss,
    Rbytes,
    Rchars,
    Rgid,
    Rgroup,
    Rops,
    Rss,
    Rtprio,
    Ruid,
    Ruser,
    S,
    Sched,
    SdMachine,
    SdOuid,
    SdSeat,
    SdSession,
    SdSlice,
    SdUnit,
    SdUunit,
    Sess,
    SgiP,
    Sgid,
    Sgroup,
    Sig,
    Sigcatch,
    Sigignore,
    Sigmask,
    Size,
    Stackp,
    Start,
    Stat,
    Stime,
    Suid,
    Supgid,
    Supgrp,
    Suser,
    Swapable,
    Sz,
    TLeft,
    TLeft2,
    TRight,
    TRight2,
    TUnlimited,
    TUnlimited2,
    Tasks,
    Time,
    Timens,
    Times,
    Tpgid,
    Trs,
    Tsig,
    Tsiz,
    Tty4,
    Tty8,
    Userns,
    Uss,
    Utilization,
    UtilizationC,
    Utsns,
    Vsz,
    Wbytes,
    Wcbytes,
    Wchan,
    Wchars,
    Wired,
    Wops,
}

/// The text of one column on one row, as a `pr_*` function returns it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Cell {
    /// The bytes.
    pub(crate) bytes: Vec<u8>,
    /// The value that the function returns, which procps-ng counts as the
    /// screen cells that the bytes take.
    pub(crate) amount: i64,
}

impl Cell {
    /// A cell whose amount is its number of bytes, as `snprintf` returns
    /// it.
    pub(crate) fn text(bytes: impl Into<Vec<u8>>) -> Self {
        let bytes = bytes.into();

        Self {
            amount: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            bytes,
        }
    }

    /// `-`, which procps-ng's `pr_nop` prints, and which the tools print for
    /// a value that they cannot read or that macOS lacks.
    pub(crate) fn dash() -> Self {
        Self::text(*b"-")
    }
}

/// What a printer needs to know about the line that `ps` is printing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Room {
    /// The screen cells that the column may take (`max_rightward`).
    pub(crate) cells: usize,
    /// Whether the line has room for signal masks of 16 digits
    /// (`wide_signals`).
    pub(crate) wide_signals: bool,
    /// procps-ng's count of rows until the next header, which its test
    /// specifiers print from.
    pub(crate) lines_to_next_header: i64,
}

/// What the printers need besides the row: the snapshot, the options and the
/// system's values.
pub(crate) struct Context<'a> {
    /// The identity of every process, which the session columns search for
    /// another member of a zombie's process group.
    identities: &'a Snapshot,
    caller: Caller,
    clock: Clock,
    /// The time that `ps` started listing, in seconds since the Unix epoch
    /// (`seconds_since_1970`).
    now: i64,
    charset: Charset,
    modifiers: Modifiers,
    date_format: Option<CString>,
    /// The installed memory in KiB, for `%mem`.
    memory_total: Option<u64>,
    /// The size of a memory page, for `sz`.
    page_size: u64,
    names: Names<'a>,
    /// Breaks a time down in the time zone that the start columns use.
    calendar: fn(i64) -> Option<CalendarTime>,
    /// The process tree, if one is shown.
    forest: Option<Forest>,
    /// The prefix of the current row in the process tree
    /// (`forest_prefix`).
    forest_prefix: Vec<Branch>,
}

/// The settings of a [`Context`] that come from the options and the system.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    /// The effective user and group of `ps`, which decide whether it may see
    /// the values that Linux shows only to a process's owner and to root.
    pub(crate) caller: Caller,
    /// The locale's character encoding.
    pub(crate) charset: Charset,
    /// The options that change how columns show their values.
    pub(crate) modifiers: Modifiers,
    /// `-D` and `--date-format`: the format of the `lstart` column.
    pub(crate) date_format: Option<Vec<u8>>,
    /// The size of a memory page.
    pub(crate) page_size: u64,
    /// Breaks a time down in the time zone that the start columns use.
    pub(crate) calendar: fn(i64) -> Option<CalendarTime>,
    /// The process tree, if one is shown.
    pub(crate) forest: Option<Forest>,
}

impl<'a> Context<'a> {
    /// The context for printing the rows of `snapshot`. `identities` has
    /// the identity of every process.
    pub(crate) fn new(
        snapshot: &'a Snapshot,
        identities: &'a Snapshot,
        settings: Settings,
        names: Names<'a>,
    ) -> Self {
        let now = snapshot
            .taken
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
            });

        Self {
            identities,
            caller: settings.caller,
            clock: Clock::new(snapshot),
            now,
            charset: settings.charset,
            modifiers: settings.modifiers,
            // A format from the command line has no NUL, because the kernel
            // ends each argument at its first NUL.
            date_format: settings
                .date_format
                .and_then(|format| CString::new(format).ok()),
            memory_total: snapshot
                .system
                .as_ref()
                .map(|system| system.memory.total / 1024),
            page_size: settings.page_size,
            names,
            calendar: settings.calendar,
            forest: settings.forest,
            forest_prefix: Vec::new(),
        }
    }

    /// Sets the prefix of the next row in the process tree, which the
    /// command columns show before the command.
    pub(crate) fn set_forest_prefix(&mut self, prefix: &[Branch]) {
        prefix.clone_into(&mut self.forest_prefix);
    }

    /// The text of the current row's prefix in the process tree, for a
    /// column with `cells` of room, or no text without a process tree.
    fn forest_text(&self, cells: usize) -> Vec<u8> {
        self.forest
            .map(|forest| forest::text(forest, &self.forest_prefix, cells))
            .unwrap_or_default()
    }

    /// The identity of every process.
    pub(crate) const fn identities(&self) -> &'a Snapshot {
        self.identities
    }

    /// The snapshot's clock.
    pub(crate) const fn clock(&self) -> Clock {
        self.clock
    }

    /// The size of a memory page.
    pub(crate) const fn page_size(&self) -> u64 {
        self.page_size
    }

    /// The locale's character encoding.
    pub(crate) const fn charset(&self) -> Charset {
        self.charset
    }

    /// The user and group names.
    pub(crate) const fn names(&mut self) -> &mut Names<'a> {
        &mut self.names
    }

    /// Whether the locale's encoding is UTF-8, which decides how `ps`
    /// escapes text.
    fn utf8(&self) -> bool {
        self.charset == Charset::Utf8
    }
}

/// The task information of `process`, if the source read it.
pub(crate) fn task_info(process: &Process) -> Option<&TaskInfo> {
    process.usage.as_ref()?.task.available()
}

/// The user and system CPU time of `task`: a thread's own times, or for a
/// whole process the times in its task information. A zombie has no task
/// information, so its times come from its resource usage.
fn cpu_times(task: &LinuxTask<'_>) -> Option<(Duration, Duration)> {
    if let Some(thread) = task.thread() {
        return Some((thread.user_time, thread.system_time));
    }

    let usage = task.process().usage.as_ref()?;

    match (usage.task.available(), usage.resources.available()) {
        (Some(task), _) => Some((task.user_time, task.system_time)),
        (None, Some(resources)) => Some((resources.user_time, resources.system_time)),
        (None, None) => None,
    }
}

/// The CPU time of `task` in clock ticks, as Linux reports `utime` plus
/// `stime` (`TICS_ALL`).
pub(crate) fn cpu_ticks(task: &LinuxTask<'_>) -> Option<u64> {
    cpu_times(task).map(|(user, system)| Clock::ticks(user) + Clock::ticks(system))
}

/// The user CPU time of `task` in clock ticks (`TICS_USER`).
pub(crate) fn user_ticks(task: &LinuxTask<'_>) -> Option<u64> {
    cpu_times(task).map(|(user, _)| Clock::ticks(user))
}

/// The CPU time of `task` and of the reaped children of its process, in
/// clock ticks (`TICS_ALL_C`). The threads of a Linux process share the
/// children's times.
fn cpu_ticks_with_children(task: &LinuxTask<'_>) -> Option<u64> {
    Some(cpu_ticks(task)? + children_ticks(task.process())?)
}

/// The ticks between the start of `process` and the snapshot, as procps-ng
/// computes them for `TIME_ELAPSED` and `UTILIZATION`, as a real number.
#[expect(
    clippy::cast_precision_loss,
    reason = "procps-ng computes the difference as a double"
)]
fn running_ticks(context: &Context<'_>, process: &Process) -> f64 {
    context.clock.now() as f64 - context.clock.tick(process.info.start_time) as f64
}

/// The seconds between the start of `process` and the snapshot
/// (`TIME_ELAPSED`), or 0 if the process started later.
#[expect(
    clippy::cast_precision_loss,
    reason = "procps-ng divides the ticks as a double"
)]
pub(crate) fn elapsed(context: &Context<'_>, process: &Process) -> f64 {
    let ticks = running_ticks(context, process);

    if ticks > 0.0 {
        ticks / Clock::HERTZ as f64
    } else {
        0.0
    }
}

/// The CPU time of `task` in seconds (`TIME_ALL`).
#[expect(
    clippy::cast_precision_loss,
    reason = "procps-ng divides the ticks as a double"
)]
pub(crate) fn cpu_seconds(task: &LinuxTask<'_>) -> Option<f64> {
    cpu_ticks(task).map(|ticks| ticks as f64 / Clock::HERTZ as f64)
}

/// The CPU time of `task` as a percentage of its process's lifetime
/// (`UTILIZATION`), or 0 if the process started at the snapshot. Darwin
/// reports no start time for a thread, so a thread's percentage is also of
/// its process's lifetime. procps-ng multiplies the ticks by 100 as a
/// `float`.
pub(crate) fn utilization(context: &Context<'_>, task: &LinuxTask<'_>) -> Option<f64> {
    Some(percentage_of_lifetime(
        context,
        task.process(),
        cpu_ticks(task)?,
    ))
}

/// The CPU time of `task`, plus the CPU time of its process's reaped
/// children, as a percentage of the process's lifetime (`UTILIZATION_C`), or
/// 0 if the process started at the snapshot.
pub(crate) fn utilization_with_children(
    context: &Context<'_>,
    task: &LinuxTask<'_>,
) -> Option<f64> {
    Some(percentage_of_lifetime(
        context,
        task.process(),
        cpu_ticks_with_children(task)?,
    ))
}

/// `ticks` as a percentage of the time since `process` started, or 0 if it
/// started at the snapshot.
#[expect(
    clippy::cast_precision_loss,
    reason = "procps-ng converts the ticks to a float"
)]
fn percentage_of_lifetime(context: &Context<'_>, process: &Process, ticks: u64) -> f64 {
    let running = running_ticks(context, process);

    if running > 0.0 {
        f64::from(ticks as f32 * 100.0) / running
    } else {
        0.0
    }
}

/// The CPU time of the children of `process` that have exited and been
/// reaped, in clock ticks, which corresponds to Linux's `cutime` plus
/// `cstime`. Darwin also counts the children that were reaped automatically,
/// because `process` ignores `SIGCHLD`.
fn children_ticks(process: &Process) -> Option<u64> {
    let resources = process.usage.as_ref()?.resources.available()?;

    Some(Clock::ticks(resources.children_user_time) + Clock::ticks(resources.children_system_time))
}

/// The user CPU time of `task` and of the reaped children of its process, in
/// clock ticks (`TICS_USER_C`).
pub(crate) fn user_ticks_with_children(task: &LinuxTask<'_>) -> Option<u64> {
    let resources = task.process().usage.as_ref()?.resources.available()?;

    Some(user_ticks(task)? + Clock::ticks(resources.children_user_time))
}

/// The virtual and resident sizes of `process` in bytes. A zombie has no
/// memory, so Linux reports 0 for both, and Darwin has no task information
/// for it.
pub(crate) fn sizes(process: &Process) -> Option<(u64, u64)> {
    if let Some(task) = task_info(process) {
        return Some((task.virtual_size, task.resident_size));
    }

    (process.info.status == Status::Zombie).then_some((0, 0))
}

/// Darwin's count of the page-ins of the process `task`, which approximates
/// Linux's count of the page faults that read from disk (`FLT_MAJ`). Darwin
/// also counts a page-in for the first fault on a page that an earlier
/// clustered read brought into memory. Darwin reports no page faults for a
/// thread, so a thread has no count.
pub(crate) fn major_faults(task: &LinuxTask<'_>) -> Option<u64> {
    if task.thread().is_some() {
        return None;
    }

    let usage = task.process().usage.as_ref()?;

    usage
        .task
        .available()
        .map(|task| u64::from(task.pageins))
        .or_else(|| usage.resources.available().map(|usage| usage.pageins))
}

/// Darwin's count of the page faults of the process `task` minus its
/// page-ins, which approximates Linux's count of the faults that did not read
/// from disk (`FLT_MIN`). A thread has no count, as for [`major_faults`].
pub(crate) fn minor_faults(task: &LinuxTask<'_>) -> Option<u64> {
    if task.thread().is_some() {
        return None;
    }

    task_info(task.process()).map(|task| u64::from(task.faults.saturating_sub(task.pageins)))
}

/// The scheduling class of `priority` as Linux numbers it (`SCHED_CLASS`):
/// 0 for `SCHED_OTHER`, 1 for `SCHED_FIFO`, 2 for `SCHED_RR`, and -1 when the
/// policy is unknown, which procps-ng treats as a kernel without policies.
pub(crate) const fn class_number(priority: &LinuxPriority) -> i32 {
    match priority.policy {
        Some(LinuxPolicy::Other) => 0,
        Some(LinuxPolicy::Fifo) => 1,
        Some(LinuxPolicy::RoundRobin) => 2,
        None => -1,
    }
}

/// The name of the terminal of `process`, as procps-ng's `dev_to_tty`
/// gives it, with `tty` removed from its start for `TTY_NUMBER`. Bytes that
/// are not printable ASCII become `?`.
pub(crate) fn terminal(process: &Process, number: bool) -> Vec<u8> {
    terminal_text(linux::terminal_name(process).as_bytes(), number)
}

/// The terminal's name `name` as [`terminal`] gives it.
fn terminal_text(mut name: &[u8], number: bool) -> Vec<u8> {
    if number && name.len() > 3 && name.starts_with(b"tty") {
        name = &name[3..];
    }

    name.iter()
        .take(64)
        .map(|&byte| {
            if (b'!'..=b'~').contains(&byte) {
                byte
            } else {
                b'?'
            }
        })
        .collect()
}

/// The signal mask `bits`, as Linux writes a mask in `/proc/<pid>/status`:
/// 16 hexadecimal digits.
pub(crate) fn signal_mask(bits: u32) -> Vec<u8> {
    format!("{bits:016x}").into_bytes()
}

/// The start of `process` in whole seconds since the Unix epoch.
pub(crate) fn start_seconds(process: &Process) -> i64 {
    process
        .info
        .start_time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// `time` as C's `ctime` writes it, such as `Thu Jan  1 00:00:00 1970\n`,
/// which is in English in every locale.
fn ctime(time: &CalendarTime) -> Vec<u8> {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let day = usize::try_from(time.day_of_week())
        .ok()
        .and_then(|day| DAYS.get(day))
        .unwrap_or(&"???");
    let month = usize::try_from(time.month())
        .ok()
        .and_then(|month| MONTHS.get(month))
        .unwrap_or(&"???");

    format!(
        "{day} {month}{:3} {:02}:{:02}:{:02} {}\n",
        time.day_of_month(),
        time.hour(),
        time.minute(),
        time.second(),
        time.year()
    )
    .into_bytes()
}

/// A number that procps-ng prints with `%d`, from an unsigned value.
#[expect(
    clippy::cast_possible_wrap,
    reason = "procps-ng prints the unsigned ID with %d"
)]
const fn signed(id: u32) -> i32 {
    id as i32
}

/// `seconds` as procps-ng's `pr_time` and `pr_etime` split them: days,
/// hours, minutes and seconds.
const fn split(seconds: u64) -> (u64, u64, u64, u64) {
    (
        seconds / 86_400,
        seconds / 3_600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    )
}

/// A real number of seconds, truncated as C converts a double to an
/// unsigned long.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "C truncates the double when it converts it to an integer"
)]
const fn whole(seconds: f64) -> u64 {
    seconds as u64
}

impl Printer {
    /// The field groups that the value comes from, with `modifiers`.
    pub(crate) fn groups(self, modifiers: Modifiers) -> &'static [FieldGroup] {
        use FieldGroup::{Arguments, Environment, FileDescriptors, Regions, Threads, Usage};

        let command_line = match self {
            Self::Args => !modifiers.contains(Modifiers::COMMAND_NAMES),
            Self::Comm => modifiers.contains(Modifiers::FULL),
            _ => false,
        };
        let environment = modifiers.contains(Modifiers::ENVIRONMENT);

        match self {
            Self::Args | Self::Comm => match (command_line, environment) {
                (true, true) => &[Arguments, Environment],
                (true, false) => &[Arguments],
                (false, true) => &[Environment],
                (false, false) => &[],
            },
            Self::Environ => &[Environment],
            Self::Fds => &[FileDescriptors],
            Self::Stat | Self::S | Self::SgiP => &[Threads],
            Self::Bsdtime
            | Self::C
            | Self::Class
            | Self::Cp
            | Self::Majflt
            | Self::Minflt
            | Self::Nice
            | Self::Nlwp
            | Self::Opri
            | Self::Pcpu
            | Self::Pmem
            | Self::Pri
            | Self::PriApi
            | Self::PriBar
            | Self::PriBaz
            | Self::PriFoo
            | Self::Priority
            | Self::Rbytes
            | Self::Rss
            | Self::Rtprio
            | Self::Sched
            | Self::Size
            | Self::Sz
            | Self::Time
            | Self::Times
            | Self::Utilization
            | Self::UtilizationC
            | Self::Vsz
            | Self::Wbytes
            | Self::ContextSwitches
            | Self::Cycles
            | Self::Energy
            | Self::Footprint
            | Self::Instructions
            | Self::MaxFootprint
            | Self::Wired => &[Usage],
            Self::Drs | Self::Dsiz | Self::Trs | Self::Tsiz | Self::Uss => &[Usage, Regions],
            Self::Swapable => &[Regions],
            _ => &[],
        }
    }

    /// Whether the value needs the system statistics.
    pub(crate) const fn needs_system(self) -> bool {
        matches!(self, Self::Pmem)
    }

    /// The text of the column for `task`, as procps-ng's function prints it.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm for each of procps-ng's functions keeps the table readable"
    )]
    pub(crate) fn print(self, context: &mut Context<'_>, task: &LinuxTask<'_>, room: Room) -> Cell {
        let process = task.process();
        let info = &process.info;
        let credentials = &info.credentials;

        match self {
            Self::Args => {
                let command_line = !context.modifiers.contains(Modifiers::COMMAND_NAMES);

                command_or_command_line(context, task, command_line, room)
            }
            Self::Comm => {
                let command_line = context.modifiers.contains(Modifiers::FULL);

                command_or_command_line(context, task, command_line, room)
            }
            Self::Fname => {
                let name = task.command(context.charset);
                let mut bytes = context.forest_text(room.cells);
                let cells = (room.cells - bytes.len()).min(8);
                let escaped = escape::escape(
                    name.as_bytes(),
                    context.utf8(),
                    cells,
                    OUTBUF_SIZE - bytes.len(),
                );
                let left = cells - escaped.cells;

                bytes.extend(escaped.bytes);

                Cell {
                    bytes,
                    amount: i64::try_from(room.cells - left).unwrap_or(i64::MAX),
                }
            }
            Self::Etime => {
                let (days, hours, minutes, seconds) = split(whole(elapsed(context, process)));
                let days_text = if days > 0 {
                    format!("{days}-")
                } else {
                    String::new()
                };
                let hours_text = if days > 0 || hours > 0 {
                    format!("{hours:02}:")
                } else {
                    String::new()
                };

                Cell::text(format!("{days_text}{hours_text}{minutes:02}:{seconds:02}"))
            }
            Self::Etimes => Cell::text(whole(elapsed(context, process)).to_string()),
            Self::C => utilization_cell(utilization(context, task), |utilization| {
                format!("{:2}", whole(utilization).min(99))
            }),
            Self::Pcpu => utilization_cell(utilization(context, task), |utilization| {
                let tenths = whole(utilization * 10.0);

                if tenths > 999 {
                    format!("{}", tenths / 10)
                } else {
                    format!("{}.{}", tenths / 10, tenths % 10)
                }
            }),
            Self::Cp => utilization_cell(utilization(context, task), |utilization| {
                format!("{:3}", whole(utilization * 10.0).min(999))
            }),
            Self::Utilization => utilization_cell(utilization(context, task), lifetime_percentage),
            Self::UtilizationC => utilization_cell(
                utilization_with_children(context, task),
                lifetime_percentage,
            ),
            Self::Pgid => Cell::text(info.pgid.as_raw().cast_unsigned().to_string()),
            Self::Ppid => Cell::text(info.ppid.as_raw().cast_unsigned().to_string()),
            Self::Time => cpu_cell(task, |seconds| {
                let (days, hours, minutes, seconds) = split(whole(seconds));
                let days = if days > 0 {
                    format!("{days}-")
                } else {
                    String::new()
                };

                format!("{days}{hours:02}:{minutes:02}:{seconds:02}")
            }),
            Self::Times => cpu_cell(task, |seconds| whole(seconds).to_string()),
            Self::Bsdtime => cpu_ticks(task).map_or_else(Cell::dash, |ticks| {
                let seconds = ticks / Clock::HERTZ;

                Cell::text(format!("{:3}:{:02}", seconds / 60, seconds % 60))
            }),
            Self::Vsz => size_cell(process, |(virtual_size, _)| virtual_size / 1024),
            Self::Rss => size_cell(process, |(_, resident)| resident / 1024),
            Self::Sz => {
                let pages = (context.page_size / 1024).max(1);

                size_cell(process, |(virtual_size, _)| virtual_size / 1024 / pages)
            }
            Self::Size => size_cell(process, |(virtual_size, _)| virtual_size),
            Self::Pmem => {
                let (Some((_, resident)), Some(total)) = (sizes(process), context.memory_total)
                else {
                    return Cell::dash();
                };
                let permille = (resident / 1024 * 1000)
                    .checked_div(total)
                    .unwrap_or(0)
                    .min(999);

                Cell::text(format!("{:2}.{}", permille / 10, permille % 10))
            }
            Self::Trs | Self::Tsiz => region_cell(context, process, true),
            Self::Drs | Self::Dsiz => region_cell(context, process, false),
            Self::Uss => number_cell(unique_size(process)),
            Self::Rbytes => number_cell(counters(process).map(|counters| counters.disk_bytes_read)),
            Self::Wbytes => {
                number_cell(counters(process).map(|counters| counters.disk_bytes_written))
            }
            Self::Fds => number_cell(open_files(process)),
            Self::Esp | Self::Eip => Cell::text(address(Some(0))),
            Self::Stackp => Cell::text(address(stack_start(context, process))),
            Self::Footprint => {
                number_cell(resources(process).map(|resources| resources.physical_footprint >> 10))
            }
            Self::MaxFootprint => number_cell(
                resources(process).map(|resources| resources.peak_physical_footprint >> 10),
            ),
            Self::Wired => {
                number_cell(resources(process).map(|resources| resources.wired_size >> 10))
            }
            Self::ContextSwitches => {
                number_cell(task_info(process).map(|task| task.context_switches))
            }
            Self::Instructions => {
                number_cell(counters(process).map(|counters| counters.instructions))
            }
            Self::Cycles => number_cell(counters(process).map(|counters| counters.cycles)),
            Self::Energy => number_cell(energy_millijoules(process)),
            Self::Architecture => Cell::text(architecture(process)),
            Self::Exe => {
                let executable = executable(context, process);

                unlimited_cell(context, &executable, room)
            }
            Self::Environ => {
                let environment = process.linux_environment_line(context.charset);

                unlimited_cell(context, environment.as_bytes(), room)
            }
            Self::Supgid => {
                let groups = supplementary_gids(process);

                Cell::text(&groups[..groups.len().min(room.cells)])
            }
            Self::Supgrp => {
                let groups = supplementary_groups(context, process);

                unlimited_cell(context, &groups, room)
            }
            Self::Swapable if has_no_memory(process) => Cell::text(*b"0"),
            Self::Swapable => process
                .linux_sizes()
                .data_and_stack
                .and_then(|size| size.available().copied())
                .map_or_else(Cell::dash, |size| Cell::text((size / 1024).to_string())),
            Self::Majflt => number_cell(major_faults(task)),
            Self::Minflt => number_cell(minor_faults(task)),
            Self::Priority => priority_cell(task, |priority| priority),
            Self::Opri => priority_cell(task, |priority| 60 + priority),
            Self::PriFoo => priority_cell(task, |priority| priority - 20),
            Self::PriBar => priority_cell(task, |priority| priority + 1),
            Self::PriBaz => priority_cell(task, |priority| priority + 100),
            Self::Pri => priority_cell(task, |priority| 39 - priority),
            Self::PriApi => priority_cell(task, |priority| -1 - priority),
            Self::Nice => {
                let priority = task.linux_priority();

                if matches!(class_number(&priority), 0 | 3 | -1) {
                    Cell::text(priority.nice.to_string())
                } else {
                    Cell::dash()
                }
            }
            Self::Class => Cell::text(task.linux_priority().class()),
            Self::Rtprio => {
                let priority = task.linux_priority();

                match (class_number(&priority), priority.rt_priority) {
                    (0 | -1, _) | (_, None) => Cell::dash(),
                    (_, Some(rt_priority)) => Cell::text(rt_priority.to_string()),
                }
            }
            Self::Sched => match class_number(&task.linux_priority()) {
                -1 => Cell::dash(),
                class => Cell::text(class.to_string()),
            },
            Self::Tty4 => Cell::text(terminal(process, true)),
            Self::Tty8 => Cell::text(terminal(process, false)),
            Self::Stat => Cell::text(task.linux_state().to_string()),
            Self::S => Cell::text(task.state_letter().unwrap_or('-').to_string()),
            Self::SgiP => match task.state_letter() {
                Some('R') | None => Cell::dash(),
                Some(_) => Cell::text(*b"*"),
            },
            Self::Flag => Cell::text(format!("{:o}", (process.linux_flags() >> 6) & 7)),
            Self::Bsdstart => start_cell(context, process, |context, start, time| {
                let ctime = ctime(&time);
                let seconds_ago = (context.now - start).max(0);
                let at = if seconds_ago > 86_400 { 4 } else { 10 };

                ctime[at..at + 6].to_vec()
            }),
            Self::Start => start_cell(context, process, |context, start, time| {
                let mut ctime = ctime(&time);

                if ctime[8] == b' ' {
                    ctime[8] = b'0';
                }

                if start + 86_400 > context.now {
                    ctime[11..19].to_vec()
                } else {
                    [b"  ".as_slice(), &ctime[4..10]].concat()
                }
            }),
            Self::Stime => start_cell(context, process, |context, _, time| {
                let Some(today) = (context.calendar)(context.now) else {
                    return Vec::new();
                };
                let format = if today.year() != time.year() {
                    c"%Y"
                } else if today.day_of_year() != time.day_of_year() {
                    c"%b%d"
                } else {
                    c"%H:%M"
                };

                time.format(format, COLWID).unwrap_or_default()
            }),
            Self::Lstart => start_cell(context, process, |context, _, time| {
                let format = context
                    .date_format
                    .as_deref()
                    .unwrap_or(DEFAULT_LSTART_FORMAT);

                time.format(format, COLWID).unwrap_or_default()
            }),
            Self::Sigcatch => signal_cell(&signal_mask(info.caught_signals.bits()), room),
            Self::Sigignore => signal_cell(&signal_mask(info.ignored_signals.bits()), room),
            Self::Euid | Self::Fuid => Cell::text(signed(credentials.euid.as_raw()).to_string()),
            Self::Ruid => Cell::text(signed(credentials.ruid.as_raw()).to_string()),
            Self::Suid => Cell::text(signed(credentials.svuid.as_raw()).to_string()),
            Self::Egid | Self::Fgid => Cell::text(signed(credentials.egid.as_raw()).to_string()),
            Self::Rgid => Cell::text(signed(credentials.rgid.as_raw()).to_string()),
            Self::Sgid => Cell::text(signed(credentials.svgid.as_raw()).to_string()),
            Self::Euser | Self::Fuser => user_cell(context, credentials.euid.as_raw(), room),
            Self::Ruser => user_cell(context, credentials.ruid.as_raw(), room),
            Self::Suser => user_cell(context, credentials.svuid.as_raw(), room),
            Self::Egroup | Self::Fgroup => group_cell(context, credentials.egid.as_raw(), room),
            Self::Rgroup => group_cell(context, credentials.rgid.as_raw(), room),
            Self::Sgroup => group_cell(context, credentials.svgid.as_raw(), room),
            Self::Procs => Cell::text(info.pid.as_raw().to_string()),
            Self::Tasks => Cell::text(task.id().to_string()),
            Self::Nlwp => process
                .thread_count()
                .map_or_else(Cell::dash, |count| Cell::text(count.to_string())),
            Self::Sess => linux::session(context.identities, process)
                .map_or_else(Cell::dash, |session| {
                    Cell::text(session.as_raw().to_string())
                }),
            Self::Tpgid => Cell::text(
                info.terminal
                    .map_or(-1, |terminal| terminal.foreground_group.as_raw())
                    .to_string(),
            ),
            Self::TLeft => test_cell(
                &["tty7", "pts/9999", "iseries/vtty42", "ttySMX0", "3270/tty4"],
                room,
                false,
            ),
            Self::TLeft2 => test_cell(&["tty7", "pts/9999", "ttySMX0", "3270/tty4"], room, false),
            Self::TRight => test_cell(
                &["999-23:59:59", "99-23:59:59", "9-23:59:59", "59:59"],
                room,
                false,
            ),
            Self::TRight2 => test_cell(&["999-23:59:59", "99-23:59:59", "9-23:59:59"], room, false),
            Self::TUnlimited => test_cell(
                &["[123456789-12345] <defunct>", "ps", "123456789-123456"],
                room,
                true,
            ),
            Self::TUnlimited2 => test_cell(
                &[
                    "unlimited",
                    "[123456789-12345] <defunct>",
                    "ps",
                    "123456789-123456",
                ],
                room,
                true,
            ),
            // procps-ng has no value for these specifiers, or they show Linux
            // concepts that macOS lacks: control groups, namespaces, systemd,
            // containers, security labels, capabilities, autogroups, the OOM
            // killer, NUMA nodes, huge pages, the processor, the wait
            // channel, the login user ID, and I/O counts that Darwin does not
            // keep. macOS has pending and blocked signals and resource limits,
            // but `kinfo_proc` reports no pending or blocked signals, and no
            // call reads another process's limits.
            Self::Agid
            | Self::Agnice
            | Self::Cgname
            | Self::Cgroup
            | Self::Cgroupns
            | Self::Context
            | Self::Docker
            | Self::Hugetblprv
            | Self::Hugetblshr
            | Self::Ipcns
            | Self::Lim
            | Self::Luid
            | Self::Lxcname
            | Self::Mntns
            | Self::Netns
            | Self::Nop
            | Self::Numa
            | Self::Oom
            | Self::OomAdj
            | Self::Pcap
            | Self::Pcaps
            | Self::Pidns
            | Self::Psr
            | Self::Pss
            | Self::Rchars
            | Self::Rops
            | Self::SdMachine
            | Self::SdOuid
            | Self::SdSeat
            | Self::SdSession
            | Self::SdSlice
            | Self::SdUnit
            | Self::SdUunit
            | Self::Sig
            | Self::Sigmask
            | Self::Timens
            | Self::Tsig
            | Self::Userns
            | Self::Utsns
            | Self::Wcbytes
            | Self::Wchan
            | Self::Wchars
            | Self::Wops => Cell::dash(),
        }
    }
}

/// The command name, or the command line, as procps-ng's
/// `pr_cmd_or_cmdline` prints it, after the prefix of a process tree. A
/// zombie's name is followed by ` <defunct>`. With BSD's `e`, the
/// environment follows, after a space, if the column has more than one cell
/// left and the environment is not `-`.
fn command_or_command_line(
    context: &Context<'_>,
    task: &LinuxTask<'_>,
    command_line: bool,
    room: Room,
) -> Cell {
    let text = if command_line {
        task.command_line(context.charset)
    } else {
        task.command(context.charset)
    };
    let prefix = context.forest_text(room.cells);
    let mut line = Line {
        rightward: room.cells - prefix.len(),
        bytes: prefix,
        utf8: context.utf8(),
    };

    line.escape(text.as_bytes());

    if !command_line && task.state_letter() == Some('Z') {
        line.escape(b" <defunct>");
    }

    if context.modifiers.contains(Modifiers::ENVIRONMENT) && line.rightward > 1 {
        let environment = task.process().linux_environment_line(context.charset);

        if environment != "-" {
            line.bytes.push(b' ');
            line.rightward -= 1;
            line.escape(environment.as_bytes());
        }
    }

    Cell {
        amount: i64::try_from(room.cells - line.rightward).unwrap_or(i64::MAX),
        bytes: line.bytes,
    }
}

/// A column's text as procps-ng builds it in its output buffer, with the
/// cells that the column has left (`rightward`).
struct Line {
    bytes: Vec<u8>,
    rightward: usize,
    utf8: bool,
}

impl Line {
    /// Appends `text`, escaped and cut to the cells that are left, as
    /// procps-ng's `escape_str` does.
    fn escape(&mut self, text: &[u8]) {
        let escaped = escape::escape(
            text,
            self.utf8,
            self.rightward,
            OUTBUF_SIZE - self.bytes.len(),
        );

        self.bytes.extend(escaped.bytes);
        self.rightward -= escaped.cells;
    }
}

/// `utilization`, formatted by `format`, or `-`.
fn utilization_cell(utilization: Option<f64>, format: impl FnOnce(f64) -> String) -> Cell {
    utilization.map_or_else(Cell::dash, |utilization| Cell::text(format(utilization)))
}

/// A percentage of a process's lifetime, as procps-ng's `pr_utilization`
/// and `pr_utilization_c` print it: with three decimals, and at most
/// 99.999.
fn lifetime_percentage(utilization: f64) -> String {
    let utilization = if utilization > 99.0 {
        99.999
    } else {
        utilization
    };

    format!("{utilization:.3}")
}

/// `text`, escaped and cut to the room of a column without a width limit,
/// as procps-ng's `escape_str` copies a string into the output buffer.
fn unlimited_cell(context: &Context<'_>, text: &[u8], room: Room) -> Cell {
    let escaped = escape::escape(text, context.utf8(), room.cells, OUTBUF_SIZE);

    Cell {
        bytes: escaped.bytes,
        amount: i64::try_from(escaped.cells).unwrap_or(i64::MAX),
    }
}

/// `number` in decimal, or `-` if the source could not read it.
fn number_cell(number: Option<impl ToString>) -> Cell {
    number.map_or_else(Cell::dash, |number| Cell::text(number.to_string()))
}

/// The resource usage of `process`, if the source read it.
pub(crate) fn resources(process: &Process) -> Option<&ResourceUsage> {
    process.usage.as_ref()?.resources.available()
}

/// The energy that `process` has used, in millijoules.
pub(crate) fn energy_millijoules(process: &Process) -> Option<u64> {
    counters(process).map(|counters| counters.energy_nanojoules / 1_000_000)
}

/// The instruction set of `process`: `x86_64` for a process that runs under
/// Rosetta, and `arm64` otherwise. Apple silicon runs no other instruction
/// set.
pub(crate) const fn architecture(process: &Process) -> &'static [u8] {
    if process.info.flags.is_translated() {
        b"x86_64"
    } else {
        b"arm64"
    }
}

/// The disk I/O, instruction, cycle and energy counters of `process`, if the
/// source read them.
pub(crate) fn counters(process: &Process) -> Option<&ResourceCounters> {
    process.usage.as_ref()?.counters.available()
}

/// The resident memory that only `process` uses, in KiB (`SMAP_PRV_TOTAL`).
/// A zombie has no memory.
pub(crate) fn unique_size(process: &Process) -> Option<u64> {
    if has_no_memory(process) {
        return Some(0);
    }

    process
        .linux_sizes()
        .unique
        .and_then(|size| size.available().copied())
        .map(|size| size >> 10)
}

/// The start of the stack of `process` (`ADDR_STACK_START`), as far as `ps`
/// knows it. Linux reports 0 to a caller that may not read the process's
/// private values and for a process without memory. Darwin does not report
/// the start of the main thread's stack, so it is unknown otherwise.
pub(crate) fn stack_start(context: &Context<'_>, process: &Process) -> Option<u64> {
    (!context.caller.may_read_private(&process.info) || has_no_memory(process)).then_some(0)
}

/// `address` as procps-ng prints an address: 16 hexadecimal digits, or `-`
/// if it is unknown.
fn address(address: Option<u64>) -> Vec<u8> {
    address.map_or_else(
        || b"-".to_vec(),
        |address| format!("{address:016x}").into_bytes(),
    )
}

/// The number of open file descriptors of `process` (`OPEN_FILES`). A zombie
/// has closed its descriptors, and Darwin does not list them.
pub(crate) fn open_files(process: &Process) -> Option<usize> {
    match process.file_descriptor_count {
        Some(Field::Available(count)) => Some(count),
        Some(Field::Unsupported) if process.info.status == Status::Zombie => Some(0),
        _ => None,
    }
}

/// The path of the executable of `process` (`EXE`), or `-` if `ps` may not
/// read the process's private values, as Linux guards `/proc/<pid>/exe`.
pub(crate) fn executable(context: &Context<'_>, process: &Process) -> Vec<u8> {
    if !context.caller.may_read_private(&process.info) {
        return b"-".to_vec();
    }

    process
        .linux_executable(context.charset)
        .into_encoded_bytes()
}

/// The group IDs of `process` separated by commas, as procps-ng's `SUPGIDS`
/// item writes Linux's `Groups` list, or `-` if the list is empty.
pub(crate) fn supplementary_gids(process: &Process) -> Vec<u8> {
    join_groups(process, |gid| gid.to_string().into_bytes())
}

/// The names of the groups of `process` separated by commas, as procps-ng's
/// `SUPGROUPS` item writes them, or `-` if the list is empty. A group without
/// a name has its ID.
pub(crate) fn supplementary_groups(context: &mut Context<'_>, process: &Process) -> Vec<u8> {
    join_groups(process, |gid| context.names.group(gid).to_vec())
}

/// The groups of `process` in ascending order of ID, each written by `write`
/// and separated by commas, or `-` if the list is empty. Linux sorts the list
/// when a process sets it, and Darwin keeps the order in which it was set.
fn join_groups(process: &Process, mut write: impl FnMut(u32) -> Vec<u8>) -> Vec<u8> {
    let mut groups: Vec<u32> = process
        .info
        .credentials
        .groups
        .iter()
        .map(|gid| gid.as_raw())
        .collect();

    if groups.is_empty() {
        return b"-".to_vec();
    }

    groups.sort_unstable();
    groups
        .into_iter()
        .map(&mut write)
        .collect::<Vec<_>>()
        .join(&b","[..])
}

/// The CPU time of `task` in seconds, formatted by `format`, or `-`.
fn cpu_cell(task: &LinuxTask<'_>, format: impl FnOnce(f64) -> String) -> Cell {
    cpu_seconds(task).map_or_else(Cell::dash, |seconds| Cell::text(format(seconds)))
}

/// A value computed by `value` from the virtual and resident sizes of
/// `process`, or `-`.
fn size_cell(process: &Process, value: impl FnOnce((u64, u64)) -> u64) -> Cell {
    sizes(process).map_or_else(Cell::dash, |sizes| Cell::text(value(sizes).to_string()))
}

/// A priority computed by `value` from Linux's `priority` field of `task`.
fn priority_cell(task: &LinuxTask<'_>, value: impl FnOnce(i32) -> i32) -> Cell {
    Cell::text(value(task.linux_priority().priority).to_string())
}

/// Whether the virtual size of `process` is 0, as it is for a zombie.
/// procps-ng prints 0 in each region column of such a process.
fn has_no_memory(process: &Process) -> bool {
    sizes(process).is_some_and(|(virtual_size, _)| virtual_size == 0)
}

/// The size of the code of `process`, or with `text` false the rest of its
/// virtual size, in KiB, as procps-ng's `pr_trs` and `pr_drs` print them.
/// Linux reports both bounds of the code as 1 to a caller that may not read
/// the process's private values, so the code is 0 and the rest is the whole
/// virtual size.
fn region_cell(context: &Context<'_>, process: &Process, text: bool) -> Cell {
    if has_no_memory(process) {
        return Cell::text(*b"0");
    }

    if !context.caller.may_read_private(&process.info) {
        if text {
            return Cell::text(*b"0");
        }

        return size_cell(process, |(virtual_size, _)| virtual_size >> 10);
    }

    let sizes = process.linux_sizes();
    let size = if text { sizes.text } else { sizes.data };

    size.and_then(|size| size.available().copied())
        .map_or_else(Cell::dash, |size| Cell::text((size >> 10).to_string()))
}

/// The cell that `format` makes from the start time of `process`, given in
/// seconds since the Unix epoch and in calendar fields, or an empty cell if
/// the start time cannot be converted to calendar fields.
fn start_cell(
    context: &Context<'_>,
    process: &Process,
    format: impl FnOnce(&Context<'_>, i64, CalendarTime) -> Vec<u8>,
) -> Cell {
    let start = start_seconds(process);

    (context.calendar)(start).map_or_else(Cell::default, |time| {
        Cell::text(format(context, start, time))
    })
}

/// A signal mask, as procps-ng's `help_pr_sig` prints it: with all 16 digits
/// when the line has room, and otherwise with the last 8, after a `<` if
/// the mask has more significant digits than that.
fn signal_cell(mask: &[u8], room: Room) -> Cell {
    let length = mask.len();

    if room.wide_signals {
        if length > 8 {
            return Cell::text(mask);
        }

        return Cell::text([b"00000000".as_slice(), mask].concat());
    }

    let zeros = mask.iter().take_while(|&&digit| digit == b'0').count();

    if length - zeros > 8 {
        return Cell::text([b"<".as_slice(), &mask[length - 8..]].concat());
    }

    if length < 8 {
        return Cell::text([&b"00000000"[length..], mask].concat());
    }

    Cell::text(&mask[length - 8..])
}

/// The user `uid`, as procps-ng's `do_pr_name` prints a name.
fn user_cell(context: &mut Context<'_>, uid: u32, room: Room) -> Cell {
    if context.modifiers.contains(Modifiers::NUMERIC) {
        return Cell::text(uid.to_string());
    }

    let utf8 = context.utf8();
    let name = context.names.user(uid).to_vec();

    name_cell(&name, uid, utf8, room)
}

/// The group `gid`, as procps-ng's `do_pr_name` prints a name.
fn group_cell(context: &mut Context<'_>, gid: u32, room: Room) -> Cell {
    if context.modifiers.contains(Modifiers::NUMERIC) {
        return Cell::text(gid.to_string());
    }

    let utf8 = context.utf8();
    let name = context.names.group(gid).to_vec();

    name_cell(&name, gid, utf8, room)
}

/// `name`, escaped, if it fits in the column. Otherwise procps-ng uses the
/// column's room in cells as an index into the escaped bytes. If the byte at
/// `room.cells - 1` is below 127, the cell is the bytes before that byte,
/// followed by `+`. Otherwise the cell is `id`.
fn name_cell(name: &[u8], id: u32, utf8: bool, room: Room) -> Cell {
    let escaped = escape::escape(name, utf8, OUTBUF_SIZE, OUTBUF_SIZE);

    if escaped.cells <= room.cells {
        return Cell {
            bytes: escaped.bytes,
            amount: i64::try_from(escaped.cells).unwrap_or(i64::MAX),
        };
    }

    if room.cells >= 1 && escaped.bytes[room.cells - 1] < 127 {
        let mut bytes = escaped.bytes[..room.cells - 1].to_vec();

        bytes.push(b'+');
        return Cell::text(bytes);
    }

    Cell::text(id.to_string())
}

/// One of `values`, as procps-ng's test specifiers choose it from the count
/// of rows until the next header. With `cut`, the value is cut to the
/// column's room.
fn test_cell(values: &[&str], room: Room, cut: bool) -> Cell {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "procps-ng converts its int count to unsigned"
    )]
    let count = room.lines_to_next_header as i32 as u32;
    let index = usize::try_from(count).unwrap_or(0) % values.len();
    let value = values[index].as_bytes();

    if cut {
        return Cell::text(&value[..value.len().min(room.cells)]);
    }

    Cell::text(value)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        ffi::OsString,
        path::PathBuf,
        time::{Duration, SystemTime},
    };

    use darwin_proc::{
        AccountingFlags, CalendarTime, Gid, LoadAverage, Memory, Pid, ProcessFlags, Protection,
        Region, ResourceCounters, ResourceUsage, RunState, SchedulingPolicy, ShareMode, SignalSet,
        Status, Swap, TaskInfo, TaskTotals, Terminal, Uid,
    };
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, Process, RegionTotals, Snapshot, System, Usage,
        helper::Caller,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{Cell, Context, Room, Settings, signal_cell, terminal_text};
    use crate::ps::{names::Names, options::Modifiers, specifier, system::testing::FakeSystem};

    /// When the snapshot is taken: 2026-10-15 12:00:00 UTC, a Thursday.
    pub(crate) const TAKEN: u64 = 1_792_065_600;

    /// The time since boot when the snapshot is taken: 30 days.
    pub(crate) const UPTIME: u64 = 30 * 86_400;

    /// The installed memory: 1 GiB.
    const MEMORY: u64 = 1 << 30;

    /// A snapshot of `processes`, with the system statistics.
    pub(crate) fn snapshot(processes: Vec<Process>) -> Snapshot {
        let memory = Memory {
            total: MEMORY,
            free: 0,
            active: 0,
            inactive: 0,
            speculative: 0,
            throttled: 0,
            wired: 0,
            purgeable: 0,
            file_backed: 0,
            anonymous: 0,
            compressor: 0,
            compressed: 0,
        };

        Snapshot {
            taken: SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN),
            uptime: Duration::from_secs(UPTIME),
            processes,
            system: Some(System {
                boot_time: SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN - UPTIME),
                load_average: LoadAverage {
                    one_minute: 0.0,
                    five_minutes: 0.0,
                    fifteen_minutes: 0.0,
                },
                memory,
                swap: Swap {
                    total: 0,
                    used: 0,
                    available: 0,
                    encrypted: false,
                },
                processors: Vec::new(),
                tasks: TaskTotals {
                    tasks: 0,
                    threads: 0,
                },
                users: 0,
            }),
        }
    }

    /// The task information of a timesharing process with two threads, one
    /// of them running, that has used 2 seconds of CPU time.
    pub(crate) const fn task() -> TaskInfo {
        TaskInfo {
            virtual_size: 4 << 30,
            resident_size: 16 << 20,
            user_time: Duration::from_millis(1_500),
            system_time: Duration::from_millis(500),
            policy: SchedulingPolicy::Timeshare,
            faults: 30,
            pageins: 10,
            copy_on_write_faults: 0,
            messages_sent: 0,
            messages_received: 0,
            mach_system_calls: 0,
            unix_system_calls: 0,
            context_switches: 0,
            threads: 2,
            running_threads: 1,
            priority: 31,
        }
    }

    /// The resource usage that goes with [`task`].
    pub(crate) const fn resources() -> ResourceUsage {
        ResourceUsage {
            user_time: Duration::from_millis(1_500),
            system_time: Duration::from_millis(500),
            runnable_time: Duration::ZERO,
            idle_wakeups: 0,
            interrupt_wakeups: 0,
            pageins: 10,
            wired_size: 0,
            resident_size: 16 << 20,
            physical_footprint: 0,
            peak_physical_footprint: 0,
            children_user_time: Duration::ZERO,
            children_system_time: Duration::ZERO,
            children_pageins: 0,
        }
    }

    /// `sleep 100`, pid 4242, which leads its session and process group on
    /// the console and has the nice value 5. It started 100 seconds before
    /// the snapshot. Its real user and group are root's, its effective user
    /// and group are `laney` and `staff`, and its saved user and group are
    /// `nobody` and 30, which has no name.
    pub(crate) fn sleeper() -> Process {
        let mut sleeper = process(4242, "sleep");
        let info = &mut sleeper.info;

        info.terminal = Some(Terminal {
            device: 0,
            foreground_group: Pid::from(4242),
        });
        info.nice = 5;
        info.session_leader = true;
        info.start_time = SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN - 100);
        info.credentials.ruid = Uid::from(0);
        info.credentials.svuid = Uid::from(4_294_967_294);
        info.credentials.rgid = Gid::from(0);
        info.credentials.svgid = Gid::from(30);
        info.caught_signals = SignalSet::from(0x4001);
        info.ignored_signals = SignalSet::from(0x1000);
        sleeper.session = Field::Available(Pid::from(4242));
        sleeper.arguments = Some(Field::Available(vec![
            OsString::from("sleep"),
            OsString::from("100"),
        ]));
        sleeper.usage = Some(Usage {
            task: Field::Available(task()),
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });
        sleeper.threads = Some(Field::Available(vec![
            thread(1, b"", RunState::Running),
            thread(2, b"", RunState::Waiting),
        ]));

        sleeper
    }

    /// A fake system that knows the user 501 as `laney`.
    pub(crate) fn system() -> FakeSystem {
        let mut system = FakeSystem::default();

        system.users.insert("laney", 501);
        system.users.insert("lan\u{e9}y", 503);
        system
    }

    /// The settings for a test, in UTF-8 with times in UTC.
    pub(crate) fn settings(modifiers: Modifiers) -> Settings {
        Settings {
            caller: Caller::new(Uid::from(0), Gid::from(0)),
            charset: Charset::Utf8,
            modifiers,
            date_format: None,
            page_size: 16_384,
            calendar: CalendarTime::utc,
            forest: None,
        }
    }

    /// The cell of the specifier `name` for `process`, with `cells` of room.
    fn printed(name: &str, process: Process, cells: usize, settings: Settings) -> Cell {
        let system = system();
        let snapshot = snapshot(vec![process]);
        let mut context = Context::new(&snapshot, &snapshot, settings, Names::new(&system));
        let specifier = specifier::find(name.as_bytes()).expect("a specifier");
        let room = Room {
            cells,
            wide_signals: true,
            lines_to_next_header: -2,
        };

        specifier
            .printer
            .print(&mut context, &snapshot.processes[0].linux_task(), room)
    }

    /// The text of the specifier `name` for `process`, with plenty of room.
    fn text(name: &str, process: Process) -> String {
        let cell = printed(name, process, 1_000, settings(Modifiers::empty()));

        String::from_utf8(cell.bytes).expect("the text is UTF-8")
    }

    #[rstest]
    #[case::pid("pid", "4242")]
    #[case::tgid("tgid", "4242")]
    #[case::lwp("lwp", "4242")]
    #[case::ppid("ppid", "1")]
    #[case::pgid("pgid", "4242")]
    #[case::sid("sid", "4242")]
    #[case::tpgid("tpgid", "4242")]
    #[case::tty("tty", "console")]
    #[case::tty4("tty4", "console")]
    #[case::uid("uid", "501")]
    #[case::euid("euid", "501")]
    #[case::ruid("ruid", "0")]
    #[case::suid("suid", "-2")]
    #[case::svuid("svuid", "-2")]
    #[case::fuid("fuid", "501")]
    #[case::fsuid("fsuid", "501")]
    #[case::user("user", "laney")]
    #[case::euser("euser", "laney")]
    #[case::uname("uname", "laney")]
    #[case::uid_hack("uid_hack", "laney")]
    #[case::ruser("ruser", "root")]
    #[case::suser("suser", "nobody")]
    #[case::svuser("svuser", "nobody")]
    #[case::fuser("fuser", "laney")]
    #[case::fsuser("fsuser", "laney")]
    #[case::gid("gid", "20")]
    #[case::egid("egid", "20")]
    #[case::rgid("rgid", "0")]
    #[case::sgid("sgid", "30")]
    #[case::svgid("svgid", "30")]
    #[case::fgid("fgid", "20")]
    #[case::fsgid("fsgid", "20")]
    #[case::group("group", "staff")]
    #[case::egroup("egroup", "staff")]
    #[case::rgroup("rgroup", "wheel")]
    #[case::sgroup("sgroup", "30")]
    #[case::svgroup("svgroup", "30")]
    #[case::fgroup("fgroup", "staff")]
    #[case::fsgroup("fsgroup", "staff")]
    #[case::ni("ni", "5")]
    #[case::pri("pri", "14")]
    #[case::opri("opri", "85")]
    #[case::priority("priority", "25")]
    #[case::pri_foo("pri_foo", "5")]
    #[case::pri_bar("pri_bar", "26")]
    #[case::pri_baz("pri_baz", "125")]
    #[case::pri_api("pri_api", "-26")]
    #[case::class("class", "TS")]
    #[case::sched("sched", "0")]
    #[case::rtprio("rtprio", "-")]
    #[case::flags("f", "0")]
    #[case::comm("comm", "sleep")]
    #[case::args("args", "sleep 100")]
    #[case::fname("fname", "sleep")]
    #[case::etime("etime", "01:40")]
    #[case::etimes("etimes", "100")]
    #[case::time("time", "00:00:02")]
    #[case::times("times", "2")]
    #[case::bsdtime("bsdtime", "  0:02")]
    #[case::pcpu("%cpu", "2.0")]
    #[case::c("c", " 2")]
    #[case::cp("cp", " 20")]
    #[case::cuu("cuu", "2.000")]
    #[case::vsz("vsz", "4194304")]
    #[case::rss("rss", "16384")]
    #[case::sz("sz", "262144")]
    #[case::m_size("m_size", "4294967296")]
    #[case::pmem("%mem", " 1.5")]
    #[case::maj_flt("maj_flt", "10")]
    #[case::min_flt("min_flt", "20")]
    #[case::stat("stat", "RNsl+")]
    #[case::state("s", "R")]
    #[case::sgi_p("sgi_p", "-")]
    #[case::nlwp("nlwp", "2")]
    #[case::caught("caught", "0000000000004001")]
    #[case::ignored("ignored", "0000000000001000")]
    #[case::pending("pending", "-")]
    #[case::blocked("blocked", "-")]
    #[case::wchan("wchan", "-")]
    #[case::psr("psr", "-")]
    #[case::label("label", "-")]
    #[case::cgroup("cgroup", "-")]
    #[case::placeholder("addr", "-")]
    #[case::stime("stime", "11:58")]
    #[case::start_time("start_time", "11:58")]
    #[case::start("start", "11:58:20")]
    #[case::bsdstart("bsdstart", " 11:58")]
    #[case::lstart("lstart", "Thu Oct 15 11:58:20 2026")]
    fn a_specifier_prints_procps_ng_s_text(#[case] name: &str, #[case] expected: &str) {
        assert_eq!(text(name, sleeper()), expected);
    }

    #[rstest]
    #[case::two_hours(7_200, ("10:00", "10:00:00", " 10:00"))]
    #[case::one_day(86_400, ("Oct14", "  Oct 14", " 12:00"))]
    #[case::other_day(2 * 86_400, ("Oct13", "  Oct 13", "Oct 13"))]
    #[case::one_digit_day(10 * 86_400, ("Oct05", "  Oct 05", "Oct  5"))]
    #[case::other_year(300 * 86_400, ("2025", "  Dec 19", "Dec 19"))]
    fn an_older_start_is_shown_by_date(#[case] age: u64, #[case] expected: (&str, &str, &str)) {
        let mut process = sleeper();
        process.info.start_time =
            SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN) - Duration::from_secs(age);

        assert_eq!(
            (
                text("stime", process.clone()),
                text("start", process.clone()),
                text("bsdstart", process)
            ),
            (
                expected.0.to_owned(),
                expected.1.to_owned(),
                expected.2.to_owned()
            )
        );
    }

    /// [`sleeper`], started `age` seconds before the snapshot.
    fn aged(age: u64) -> Process {
        let mut process = sleeper();
        process.info.start_time =
            SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN) - Duration::from_secs(age);

        process
    }

    /// [`sleeper`], started `age` seconds before the snapshot, after it used
    /// `cpu` of CPU time.
    fn busy(age: u64, cpu: Duration) -> Process {
        let mut process = aged(age);
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

    #[rstest]
    #[case::minutes(100, ("01:40", "100"))]
    #[case::hours(7_205, ("02:00:05", "7205"))]
    #[case::one_day(86_405, ("1-00:00:05", "86405"))]
    #[case::days(3 * 86_400 + 3_600, ("3-01:00:00", "262800"))]
    fn the_elapsed_time_shows_days_and_hours_when_it_has_them(
        #[case] age: u64,
        #[case] expected: (&str, &str),
    ) {
        assert_eq!(
            (text("etime", aged(age)), text("etimes", aged(age))),
            (expected.0.to_owned(), expected.1.to_owned())
        );
    }

    #[rstest]
    #[case::hours(Duration::from_secs(7_205), ("02:00:05", "120:05"))]
    #[case::days(Duration::from_hours(25), ("1-01:00:00", "1500:00"))]
    fn the_cpu_time_shows_days_when_it_has_them(
        #[case] cpu: Duration,
        #[case] expected: (&str, &str),
    ) {
        let process = busy(100_000, cpu);

        assert_eq!(
            (text("time", process.clone()), text("bsdtime", process)),
            (expected.0.to_owned(), expected.1.to_owned())
        );
    }

    #[rstest]
    #[case::two_and_a_half(Duration::from_secs(250), ("250", "99", "999", "99.999"))]
    #[case::ten(Duration::from_secs(1_000), ("1000", "99", "999", "99.999"))]
    fn cpu_use_above_100_percent_has_procps_ng_s_limits(
        #[case] cpu: Duration,
        #[case] expected: (&str, &str, &str, &str),
    ) {
        let process = busy(100, cpu);

        assert_eq!(
            (
                text("%cpu", process.clone()),
                text("c", process.clone()),
                text("cp", process.clone()),
                text("cuu", process)
            ),
            (
                expected.0.to_owned(),
                expected.1.to_owned(),
                expected.2.to_owned(),
                expected.3.to_owned()
            )
        );
    }

    #[test]
    fn memory_use_is_limited_to_99_point_9_percent() {
        let mut process = sleeper();
        process.usage = Some(Usage {
            task: Field::Available(TaskInfo {
                resident_size: 2 << 30,
                ..task()
            }),
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });

        assert_eq!(text("%mem", process), "99.9");
    }

    #[test]
    fn a_zombie_s_name_is_cut_to_its_room_with_defunct() {
        let mut process = sleeper();
        process.info.status = Status::Zombie;

        assert_eq!(
            printed("comm", process, 10, settings(Modifiers::empty())),
            Cell {
                bytes: b"sleep <def".to_vec(),
                amount: 10
            }
        );
    }

    #[rstest]
    #[case::waiting(RunState::Waiting, "*")]
    #[case::running(RunState::Running, "-")]
    fn sgi_p_shows_a_star_for_a_process_that_is_not_running(
        #[case] state: RunState,
        #[case] expected: &str,
    ) {
        let mut process = sleeper();
        process.threads = Some(Field::Available(vec![thread(1, b"", state)]));

        assert_eq!(text("sgi_p", process), expected);
    }

    #[test]
    fn a_process_without_a_terminal_has_the_foreground_group_minus_1() {
        let mut process = sleeper();
        process.info.terminal = None;

        assert_eq!(
            (text("tpgid", process.clone()), text("tty", process)),
            ("-1".to_owned(), "?".to_owned())
        );
    }

    #[rstest]
    #[case::left("_left", 1_000, "3270/tty4")]
    #[case::left2("_left2", 1_000, "ttySMX0")]
    #[case::right("_right", 1_000, "9-23:59:59")]
    #[case::right2("_right2", 1_000, "9-23:59:59")]
    #[case::unlimited("_unlimited", 5, "12345")]
    #[case::unlimited2("_unlimited2", 5, "ps")]
    fn a_test_specifier_chooses_its_value_from_the_header_count(
        #[case] name: &str,
        #[case] cells: usize,
        #[case] expected: &str,
    ) {
        assert_eq!(
            printed(name, sleeper(), cells, settings(Modifiers::empty())),
            Cell::text(expected)
        );
    }

    #[test]
    fn values_that_the_source_could_not_read_are_dashes() {
        let mut process = sleeper();
        process.usage = Some(Usage {
            task: Field::Denied,
            resources: Field::Denied,
            counters: Field::Denied,
        });
        process.threads = Some(Field::Denied);
        process.arguments = Some(Field::Denied);
        process.session = Field::Denied;

        let names = ["time", "%cpu", "vsz", "rss", "stat", "nlwp", "args", "sid"];
        let printed: Vec<String> = names
            .iter()
            .map(|name| text(name, process.clone()))
            .collect();

        assert_eq!(printed, ["-", "-", "-", "-", "-Ns+", "-", "[sleep]", "-"]);
    }

    /// [`sleeper`], whose real, effective and saved IDs are all `uid` and
    /// `gid`, with 64 KiB of code in executable regions and 1 MiB of private
    /// writable regions.
    fn owned_by(uid: u32, gid: u32) -> Process {
        let mut process = sleeper();
        let credentials = &mut process.info.credentials;

        [credentials.ruid, credentials.euid, credentials.svuid] = [Uid::from(uid); 3];
        [credentials.rgid, credentials.egid, credentials.svgid] = [Gid::from(gid); 3];
        process.region_totals = Some(Field::Available(RegionTotals {
            executable: 64 << 10,
            private_writable: 1 << 20,
        }));

        process
    }

    // Linux shows the bounds of a process's code only to a caller that passes
    // its ptrace read-access check, and reports both bounds as 1 otherwise.
    #[rstest]
    #[case::owner(owned_by(501, 20), ["64", "4194240", "64", "4194240"])]
    #[case::other_user(owned_by(502, 20), ["0", "4194304", "0", "4194304"])]
    #[case::other_group(owned_by(501, 12), ["0", "4194304", "0", "4194304"])]
    #[case::mixed_ids(sleeper(), ["0", "4194304", "0", "4194304"])]
    fn only_a_process_that_ps_may_read_shows_its_code(
        #[case] process: Process,
        #[case] expected: [&str; 4],
    ) {
        let settings = Settings {
            caller: Caller::new(Uid::from(501), Gid::from(20)),
            ..settings(Modifiers::empty())
        };
        let printed = ["trs", "drs", "tsiz", "dsiz"].map(|name| {
            let cell = printed(name, process.clone(), 1_000, settings.clone());

            String::from_utf8(cell.bytes).expect("the text is UTF-8")
        });

        assert_eq!(printed, expected);
    }

    /// [`owned_by`] the user 501 and the group 20, with the values that the
    /// columns of Linux's private files come from.
    fn with_private_values() -> Process {
        let mut process = owned_by(501, 20);

        process.executable = Field::Available(PathBuf::from("/bin/sleep"));
        process.environment = Some(Field::Available(vec![
            OsString::from("HOME=/"),
            OsString::from("TERM=vt100"),
        ]));
        process.info.credentials.groups = [20, 0, 30].map(Gid::from).to_vec();
        process = process.with_file_descriptors(Field::Available(vec![0, 1, 2, 5]));
        process.usage = Some(Usage {
            task: Field::Available(task()),
            resources: Field::Available(ResourceUsage {
                children_user_time: Duration::from_secs(4),
                children_system_time: Duration::from_secs(2),
                ..resources()
            }),
            counters: Field::Available(ResourceCounters {
                disk_bytes_read: 4_096,
                disk_bytes_written: 8_192,
                logical_writes: 0,
                instructions: 0,
                cycles: 0,
                energy_nanojoules: 0,
            }),
        });
        process.regions = Some(Field::Available(vec![
            region(48 << 10, 0),
            region(16 << 10, 1),
        ]));

        process
    }

    /// A region with `private_resident` bytes that only its process uses,
    /// and `shared_resident` bytes that other processes may use too.
    fn region(private_resident: u64, shared_resident: u64) -> Region {
        Region {
            address: 0,
            size: 1 << 20,
            protection: Protection::from(0x3),
            max_protection: Protection::from(0x7),
            share_mode: ShareMode::Private,
            user_tag: 0,
            resident: private_resident + shared_resident,
            private_resident,
            shared_resident,
            shared_now_private: 0,
            swapped_out: 0,
            dirtied: 0,
            reference_count: 1,
            object_id: None,
            is_submap: false,
            is_shared: false,
        }
    }

    #[rstest]
    #[case::exe("exe", "/bin/sleep")]
    #[case::environ("environ", "HOME=/ TERM=vt100")]
    #[case::supgid("supgid", "0,20,30")]
    #[case::supgrp("supgrp", "wheel,staff,30")]
    #[case::fds("fds", "4")]
    #[case::lim("lim", "-")]
    #[case::rbytes("rbytes", "4096")]
    #[case::wbytes("wbytes", "8192")]
    #[case::uss("uss", "64")]
    #[case::cuc("cuc", "8.000")]
    fn a_private_value_is_shown_to_the_owner(#[case] name: &str, #[case] expected: &str) {
        let settings = Settings {
            caller: Caller::new(Uid::from(501), Gid::from(20)),
            ..settings(Modifiers::empty())
        };
        let cell = printed(name, with_private_values(), 1_000, settings);

        assert_eq!(
            String::from_utf8(cell.bytes).expect("the text is UTF-8"),
            expected
        );
    }

    // The helper withholds the environment, the descriptors, the regions and
    // the counters from other users, and macOS withholds them without the
    // helper. The executable's path reaches every user, so `ps` applies the
    // access check to it itself, as Linux guards `/proc/<pid>/exe`. The
    // helper sends the number of descriptors to every user.
    #[test]
    fn a_private_value_is_a_dash_for_another_user() {
        let mut process = with_private_values();
        process.environment = Some(Field::Denied);
        process.file_descriptors = Some(Field::Denied);
        process.regions = Some(Field::Denied);
        process.usage = Some(Usage {
            counters: Field::Denied,
            ..process.usage.expect("the usage")
        });
        let settings = Settings {
            caller: Caller::new(Uid::from(502), Gid::from(20)),
            ..settings(Modifiers::empty())
        };
        let printed: Vec<String> = ["exe", "environ", "fds", "rbytes", "wbytes", "uss"]
            .iter()
            .map(|name| {
                let cell = printed(name, process.clone(), 1_000, settings.clone());

                String::from_utf8(cell.bytes).expect("the text is UTF-8")
            })
            .collect();

        assert_eq!(printed, ["-", "-", "4", "-", "-", "-"]);
    }

    // Linux reports the stack pointer and the instruction pointer only for a
    // task that is exiting or dumping core, and 0 otherwise. It reports the
    // start of the stack only to a caller that passes ptrace's read-access
    // check, and 0 for a process without memory.
    #[rstest]
    #[case::reader(owned_by(501, 20), ["-", "0000000000000000", "0000000000000000"])]
    #[case::other_user(owned_by(502, 20), ZEROS)]
    #[case::zombie(zombie_of(owned_by(501, 20)), ZEROS)]
    fn the_address_columns_show_what_linux_shows(
        #[case] process: Process,
        #[case] expected: [&str; 3],
    ) {
        let settings = Settings {
            caller: Caller::new(Uid::from(501), Gid::from(20)),
            ..settings(Modifiers::empty())
        };
        let printed = ["stackp", "esp", "eip"].map(|name| {
            let cell = printed(name, process.clone(), 1_000, settings.clone());

            String::from_utf8(cell.bytes).expect("the text is UTF-8")
        });

        assert_eq!(printed, expected);
    }

    /// The address columns of a process whose addresses Linux reports as 0.
    const ZEROS: [&str; 3] = ["0000000000000000"; 3];

    /// `process` as a zombie, without task information.
    fn zombie_of(mut process: Process) -> Process {
        process.info.status = Status::Zombie;
        process.usage = Some(Usage {
            task: Field::Unsupported,
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });

        process
    }

    #[rstest]
    #[case::cut(3, "0,2")]
    #[case::fits(7, "0,20,30")]
    fn supgid_is_cut_to_its_room(#[case] cells: usize, #[case] expected: &str) {
        assert_eq!(
            printed(
                "supgid",
                with_private_values(),
                cells,
                settings(Modifiers::empty())
            ),
            Cell::text(expected)
        );
    }

    #[rstest]
    #[case::escaped(1_000, "staff,a?b", 9)]
    #[case::cut(8, "staff,a?", 8)]
    fn supgrp_is_escaped_and_cut_to_its_room(
        #[case] cells: usize,
        #[case] expected: &str,
        #[case] amount: i64,
    ) {
        let mut process = with_private_values();
        process.info.credentials.groups = [40, 20].map(Gid::from).to_vec();
        let mut system = system();
        system.groups.insert("a\tb", 40);
        let snapshot = snapshot(vec![process]);
        let mut context = Context::new(
            &snapshot,
            &snapshot,
            settings(Modifiers::empty()),
            Names::new(&system),
        );
        let room = Room {
            cells,
            wide_signals: true,
            lines_to_next_header: -2,
        };
        let specifier = specifier::find(b"supgrp").expect("a specifier");

        assert_eq!(
            specifier
                .printer
                .print(&mut context, &snapshot.processes[0].linux_task(), room),
            Cell {
                bytes: expected.as_bytes().to_vec(),
                amount
            }
        );
    }

    // procps-ng leaves the percentage at 0 when the process started at the
    // snapshot, as it does for `pcpu`.
    #[test]
    fn cuc_of_a_process_that_started_at_the_snapshot_is_0() {
        let mut process = with_private_values();
        process.info.start_time = SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN);

        assert_eq!(text("cuc", process), "0.000");
    }

    /// [`sleeper`] with the values that the Darwin columns show.
    fn with_darwin_values() -> Process {
        let mut process = sleeper();

        process.usage = Some(Usage {
            task: Field::Available(TaskInfo {
                context_switches: 42,
                ..task()
            }),
            resources: Field::Available(ResourceUsage {
                physical_footprint: 3 << 20,
                peak_physical_footprint: 5 << 20,
                wired_size: 64 << 10,
                ..resources()
            }),
            counters: Field::Available(ResourceCounters {
                disk_bytes_read: 0,
                disk_bytes_written: 0,
                logical_writes: 0,
                instructions: 1_000,
                cycles: 2_000,
                energy_nanojoules: 5_000_000_000,
            }),
        });

        process
    }

    #[rstest]
    #[case::footprint("footprint", "3072")]
    #[case::peak_footprint("maxfootprint", "5120")]
    #[case::wired("wired", "64")]
    #[case::context_switches("csw", "42")]
    #[case::instructions("instrs", "1000")]
    #[case::cycles("cycles", "2000")]
    #[case::energy("energy", "5000")]
    #[case::architecture("arch", "arm64")]
    fn a_darwin_column_shows_its_value(#[case] name: &str, #[case] expected: &str) {
        assert_eq!(text(name, with_darwin_values()), expected);
    }

    #[test]
    fn a_process_under_rosetta_is_x86_64() {
        let mut process = sleeper();
        process.info.flags = ProcessFlags::from(0x2_0000);

        assert_eq!(text("arch", process), "x86_64");
    }

    #[test]
    fn a_darwin_value_that_the_source_could_not_read_is_a_dash() {
        let mut process = sleeper();
        process.usage = Some(Usage {
            task: Field::Denied,
            resources: Field::Denied,
            counters: Field::Denied,
        });

        let printed: Vec<String> = [
            "footprint",
            "maxfootprint",
            "wired",
            "csw",
            "instrs",
            "cycles",
            "energy",
        ]
        .iter()
        .map(|name| text(name, process.clone()))
        .collect();

        assert_eq!(printed, ["-", "-", "-", "-", "-", "-", "-"]);
    }

    #[rstest]
    #[case::fits(18, "HOME=/ TERM=vt100", 17)]
    #[case::cut(6, "HOME=/", 6)]
    fn the_environment_is_cut_to_its_room(
        #[case] cells: usize,
        #[case] expected: &str,
        #[case] amount: i64,
    ) {
        assert_eq!(
            printed(
                "environ",
                with_private_values(),
                cells,
                settings(Modifiers::empty())
            ),
            Cell {
                bytes: expected.as_bytes().to_vec(),
                amount
            }
        );
    }

    #[test]
    fn a_process_without_groups_has_a_dash() {
        let mut process = with_private_values();
        process.info.credentials.groups = Vec::new();

        assert_eq!(
            (text("supgid", process.clone()), text("supgrp", process)),
            ("-".to_owned(), "-".to_owned())
        );
    }

    #[test]
    fn a_zombie_has_no_open_files_or_memory() {
        let mut process = with_private_values();
        process.info.status = Status::Zombie;
        process.executable = Field::Unsupported;
        process = process.with_file_descriptors(Field::Unsupported);
        process.regions = Some(Field::Unsupported);
        process.usage = Some(Usage {
            task: Field::Unsupported,
            ..process.usage.expect("the usage")
        });

        let printed: Vec<String> = ["exe", "fds", "uss", "rbytes"]
            .iter()
            .map(|name| text(name, process.clone()))
            .collect();

        assert_eq!(printed, ["-", "0", "0", "4096"]);
    }

    #[test]
    fn a_zombie_has_no_memory_and_is_defunct() {
        let mut process = sleeper();
        process.info.status = Status::Zombie;
        process.usage = Some(Usage {
            task: Field::Failed,
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });
        process.threads = Some(Field::Failed);
        process.arguments = Some(Field::Unsupported);
        process.region_totals = Some(Field::Failed);
        process.regions = Some(Field::Failed);

        let names = [
            "vsz", "rss", "sz", "%mem", "trs", "drs", "tsiz", "dsiz", "size", "nlwp", "class",
            "sched", "ni", "min_flt", "time", "comm", "args", "stat",
        ];
        let printed: Vec<String> = names
            .iter()
            .map(|name| text(name, process.clone()))
            .collect();

        assert_eq!(
            printed,
            [
                "0",
                "0",
                "0",
                " 0.0",
                "0",
                "0",
                "0",
                "0",
                "0",
                "1",
                "TS",
                "0",
                "5",
                "-",
                "00:00:02",
                "sleep <defunct>",
                "[sleep] <defunct>",
                "ZNs+"
            ]
        );
    }

    #[rstest]
    #[case::fifo(SchedulingPolicy::Fifo, ("-", "FF", "1", "31", "-32"))]
    #[case::round_robin(SchedulingPolicy::RoundRobin, ("-", "RR", "2", "31", "-32"))]
    fn a_fixed_priority_process_has_a_real_time_priority(
        #[case] policy: SchedulingPolicy,
        #[case] expected: (&str, &str, &str, &str, &str),
    ) {
        let mut process = sleeper();
        process.usage = Some(Usage {
            task: Field::Available(TaskInfo { policy, ..task() }),
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });

        assert_eq!(
            (
                text("ni", process.clone()),
                text("class", process.clone()),
                text("sched", process.clone()),
                text("rtprio", process.clone()),
                text("priority", process)
            ),
            (
                expected.0.to_owned(),
                expected.1.to_owned(),
                expected.2.to_owned(),
                expected.3.to_owned(),
                expected.4.to_owned()
            )
        );
    }

    #[test]
    fn modifiers_change_the_command_and_user_columns() {
        let printed = |name: &str, modifiers: Modifiers| {
            let cell = printed(name, sleeper(), 1_000, settings(modifiers));

            String::from_utf8(cell.bytes).expect("the text is UTF-8")
        };

        assert_eq!(
            (
                printed("args", Modifiers::COMMAND_NAMES),
                printed("comm", Modifiers::FULL),
                printed("user", Modifiers::NUMERIC),
                printed("group", Modifiers::NUMERIC)
            ),
            (
                "sleep".to_owned(),
                "sleep 100".to_owned(),
                "501".to_owned(),
                "20".to_owned()
            )
        );
    }

    // procps-ng appends the environment only when the column has more than
    // one cell left after the command, and leaves out an environment that it
    // could not read.
    #[rstest]
    #[case::command_line("args", with_private_values(), 1_000, Cell::text(*b"sleep 100 HOME=/ TERM=vt100"))]
    #[case::command_name("comm", with_private_values(), 1_000, Cell::text(*b"sleep HOME=/ TERM=vt100"))]
    #[case::cut("args", with_private_values(), 12, Cell::text(*b"sleep 100 HO"))]
    #[case::one_cell_left("args", with_private_values(), 10, Cell { bytes: b"sleep 100".to_vec(), amount: 9 })]
    #[case::unreadable("args", sleeper(), 1_000, Cell::text(*b"sleep 100"))]
    fn the_environment_follows_the_command(
        #[case] name: &str,
        #[case] process: Process,
        #[case] cells: usize,
        #[case] expected: Cell,
    ) {
        assert_eq!(
            printed(name, process, cells, settings(Modifiers::ENVIRONMENT)),
            expected
        );
    }

    #[rstest]
    #[case::fits(501, 5, Cell::text(*b"laney"))]
    #[case::cut(501, 4, Cell::text(*b"lan+"))]
    #[case::cut_after_ascii(503, 3, Cell::text(*b"la+"))]
    #[case::cut_in_a_character(503, 4, Cell::text(*b"503"))]
    fn a_user_name_is_cut_to_its_column(
        #[case] uid: u32,
        #[case] cells: usize,
        #[case] expected: Cell,
    ) {
        let mut process = sleeper();
        process.info.credentials.euid = Uid::from(uid);

        assert_eq!(
            printed("user", process, cells, settings(Modifiers::empty())),
            expected
        );
    }

    #[rstest]
    #[case::fits(27, "sleep 100", 9)]
    #[case::cut(4, "slee", 4)]
    fn a_command_line_is_cut_to_its_room(
        #[case] cells: usize,
        #[case] expected: &str,
        #[case] amount: i64,
    ) {
        assert_eq!(
            printed("args", sleeper(), cells, settings(Modifiers::empty())),
            Cell {
                bytes: expected.as_bytes().to_vec(),
                amount
            }
        );
    }

    #[test]
    fn fname_reports_the_room_that_procps_ng_reports() {
        assert_eq!(
            printed("fname", sleeper(), 15, settings(Modifiers::empty())),
            Cell {
                bytes: b"sleep".to_vec(),
                amount: 12
            }
        );
    }

    #[test]
    fn the_date_format_sets_the_format_of_lstart() {
        let settings = Settings {
            date_format: Some(b"%Y-%m-%d".to_vec()),
            ..settings(Modifiers::empty())
        };

        assert_eq!(
            printed("lstart", sleeper(), 1_000, settings),
            Cell::text(*b"2026-10-15")
        );
    }

    #[rstest]
    #[case::wide(true, b"0000000100004001", "0000000100004001")]
    #[case::narrow(false, b"0000000000004001", "00004001")]
    #[case::narrow_with_more_digits(false, b"0000000100004001", "<00004001")]
    #[case::short_narrow(false, b"4001", "00004001")]
    #[case::short_wide(true, b"4001", "000000004001")]
    fn a_signal_mask_is_cut_as_procps_ng_cuts_it(
        #[case] wide: bool,
        #[case] mask: &[u8],
        #[case] expected: &str,
    ) {
        let room = Room {
            cells: 9,
            wide_signals: wide,
            lines_to_next_header: -2,
        };

        assert_eq!(signal_cell(mask, room), Cell::text(expected));
    }

    #[rstest]
    #[case::wide(true, "0000000000004001")]
    #[case::narrow(false, "00004001")]
    fn a_signal_mask_has_16_digits_only_on_a_wide_line(#[case] wide: bool, #[case] expected: &str) {
        let system = system();
        let snapshot = snapshot(vec![sleeper()]);
        let mut context = Context::new(
            &snapshot,
            &snapshot,
            settings(Modifiers::empty()),
            Names::new(&system),
        );
        let room = Room {
            cells: 9,
            wide_signals: wide,
            lines_to_next_header: -2,
        };
        let specifier = specifier::find(b"caught").expect("a specifier");

        assert_eq!(
            specifier
                .printer
                .print(&mut context, &snapshot.processes[0].linux_task(), room),
            Cell::text(expected)
        );
    }

    #[rstest]
    #[case::terminal(b"ttys001", true, b"s001")]
    #[case::name(b"ttys001", false, b"ttys001")]
    #[case::only_tty(b"tty", true, b"tty")]
    #[case::console(b"console", true, b"console")]
    #[case::unprintable(b"a b\x7f", false, b"a?b?")]
    fn a_terminal_name_is_abbreviated_as_procps_ng_abbreviates_it(
        #[case] name: &[u8],
        #[case] number: bool,
        #[case] expected: &[u8],
    ) {
        assert_eq!(terminal_text(name, number), expected);
    }

    #[test]
    fn a_process_without_accounting_flags_has_no_linux_flags() {
        let mut process = sleeper();
        process.info.accounting = AccountingFlags::from(0x3);

        assert_eq!(text("f", process), "5");
    }
}
