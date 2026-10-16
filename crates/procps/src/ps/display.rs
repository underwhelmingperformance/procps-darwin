// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/display.c.

use std::{collections::BTreeSet, io::Write};

use darwin_proc::{CalendarTime, Gid, Pid, Status, Uid};
use procps_core::{
    Charset, FieldGroup, LinuxTask, ProcessSource, Snapshot, SnapshotRequest, SourceError,
    helper::Caller,
};

use crate::ps::{
    error::{PsError, Unimplemented},
    forest::{self, Branch},
    format::{Content, Order},
    layout::{self, Layout},
    names::Names,
    options::{Modifiers, Options, ThreadFlags},
    print::{self, Cell, Context},
    selection::Myself,
    sort,
    specifier::Print,
    system::System,
};

/// What the listing needs to know about `ps` itself and the system.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Invocation {
    /// The process of `ps`, whose terminal the default selection uses.
    pub(crate) myself: Pid,
    /// The effective user ID of `ps`.
    pub(crate) euid: u32,
    /// The effective group ID of `ps`.
    pub(crate) egid: u32,
    /// The locale's character encoding.
    pub(crate) charset: Charset,
    /// The size of a memory page.
    pub(crate) page_size: u64,
    /// Breaks a time down in the time zone that the start columns use.
    pub(crate) calendar: fn(i64) -> Option<CalendarTime>,
}

/// The first part of `options` that this project does not implement yet.
fn unimplemented(options: &Options) -> Option<Unimplemented> {
    let modifiers = [
        (Modifiers::ENVIRONMENT, "e"),
        (Modifiers::CUMULATIVE, "S"),
        (Modifiers::SIGNAL_NAMES, "--signames"),
    ];

    if let Some(&(_, option)) = modifiers
        .iter()
        .find(|(modifier, _)| options.modifiers.contains(*modifier))
    {
        return Some(Unimplemented::Option(option));
    }

    None
}

/// The rows of a listing, as procps-ng's `simple_spew` and `fancy_spew`
/// decide them from the thread display.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Rows {
    /// A row for each process.
    Processes,
    /// A row for each thread: `-L`, `-T` and BSD's `H`.
    Threads,
    /// A row for each process, followed by a row for each of its threads:
    /// BSD's `m` and UNIX's `-m`.
    ProcessesAndThreads,
}

impl Rows {
    /// The rows that `options` ask for. procps-ng applies the sort options,
    /// and shows a process tree, in `fancy_spew`, which reads the threads only
    /// for BSD's `H`. A sorted listing with `-L`, `-T`, `m` or `-m` therefore
    /// has a row for each process.
    fn of(options: &Options) -> Self {
        let threads = options.threads;
        let fancy = options.forest.is_some() || !options.sort.is_empty();

        if threads.contains(ThreadFlags::LOOSE_TASKS) {
            return Self::Threads;
        }

        if fancy || !threads.contains(ThreadFlags::SHOW_TASK) {
            return Self::Processes;
        }

        if threads.contains(ThreadFlags::SHOW_PROCESS) {
            Self::ProcessesAndThreads
        } else {
            Self::Threads
        }
    }
}

/// What a row shows, which decides the columns that it leaves out when `ps`
/// shows processes and their threads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RowKind {
    /// A process.
    Process,
    /// A thread.
    Thread,
}

impl RowKind {
    /// Whether a row of this kind shows `-` in place of a column with
    /// `print`, as procps-ng's `lists_and_needs` replaces the column's printer
    /// with `pr_nop` when `ps` shows processes and their threads.
    const fn hides(self, print: Print) -> bool {
        matches!(
            (self, print),
            (Self::Process, Print::ThreadOnly) | (Self::Thread, Print::ProcessOnly)
        )
    }
}

/// The request for the data that `options` show: the field groups that the
/// columns, the sort keys and the selection need, the threads for rows of
/// threads, and the system statistics for `%mem`. `ps` asks for the data only
/// for the processes that the selection may select. The request leaves out
/// the threads when `threads_read` says that the first snapshot has read them.
fn data_request(options: &Options, threads_read: bool) -> SnapshotRequest {
    let printers = options
        .columns
        .iter()
        .filter_map(|column| match column.content {
            Content::Specifier(specifier) => Some(specifier.printer),
            Content::Text => None,
        });
    let threads: &[FieldGroup] = match Rows::of(options) {
        Rows::Processes => &[],
        Rows::Threads | Rows::ProcessesAndThreads => &[FieldGroup::Threads],
    };
    let groups: BTreeSet<_> = printers
        .clone()
        .flat_map(|printer| printer.groups(options.modifiers))
        .chain(
            options
                .sort
                .iter()
                .filter(|key| key.order != Order::Ignored)
                .flat_map(|key| key.specifier.sort.groups()),
        )
        .chain(options.selection.groups())
        .chain(threads)
        .copied()
        .filter(|&group| !(threads_read && group == FieldGroup::Threads))
        .collect();
    let request = groups
        .into_iter()
        .fold(SnapshotRequest::default(), SnapshotRequest::with);

    if printers.clone().any(print::Printer::needs_system) {
        return request.with_system();
    }

    request
}

/// The processes that a listing may show.
#[derive(Debug)]
struct Listing {
    /// The identity of every process. The selection and the session columns
    /// read it.
    identities: Snapshot,
    /// The data of the processes that the selection may select, or `None` if
    /// the columns, the sort keys and the selection need only the identity.
    data: Option<Snapshot>,
    /// The processes that the selection may select, as indices into the
    /// data's processes, in the order that procps-ng reads them.
    candidates: Vec<usize>,
    /// What the selection needs to know about `ps` itself.
    myself: Myself,
}

impl Listing {
    /// The snapshot that the candidates index.
    fn data(&self) -> &Snapshot {
        self.data.as_ref().unwrap_or(&self.identities)
    }
}

/// The index of the process `pid` in `snapshot`, whose processes are in
/// ascending order of pid.
fn find(snapshot: &Snapshot, pid: Pid) -> Option<usize> {
    snapshot
        .processes
        .binary_search_by_key(&pid, |process| process.info.pid)
        .ok()
}

/// Reads the processes that `options` may select from `source`. The first
/// snapshot has the identity of every process. If the columns, the sort
/// keys, the selection or the rows need more than the identity, a second
/// snapshot reads that data. It reads only the processes that the selection
/// may select according to the first snapshot, because a walk of the memory
/// regions of every process takes seconds. When the rows show threads and
/// the selection compares the names of threads, the first snapshot also
/// reads the threads, and a process may be selected if one of its tasks may
/// be. The second snapshot then does not read the threads again, and each
/// candidate keeps the threads from the first, unless it has become a
/// zombie, which has no threads on Darwin. A zombie keeps its start time, so
/// it stays a candidate. A process that exits and is reaped between the two
/// snapshots is left out, and so is one whose pid another process has taken
/// by then.
///
/// The candidates are in the order that procps-ng reads the processes: the
/// order of the list for a selection that reads its processes directly, such
/// as `-q` or a `-p` on its own, and otherwise ascending order of pid, as
/// Linux's `/proc` lists them. Linux does not show its idle tasks, which have
/// pid 0, in `/proc`, so procps-ng never lists a process with pid 0. `ps`
/// leaves out `kernel_task`, which has pid 0 on Darwin.
///
/// When the rows show threads, repetitions of a pid next to each other in
/// `-q` give one listing of the process's threads, because procps-ng's
/// `simple_nexttid` reads the threads of a process again only after another
/// process. Another process that exists between the repetitions makes
/// procps-ng list the threads again. For a list such as `A,X,A`, where X does
/// not exist, procps-ng 4.0.7 crashes with `SIGSEGV`: the failed opening of
/// X's `task` directory sets `taskdir` to `NULL`, but leaves `taskdir_user`
/// set to A, so the second A calls `readdir(NULL)`. `ps` deliberately lists
/// A's threads once.
fn read(
    options: &Options,
    invocation: &Invocation,
    source: &impl ProcessSource,
) -> Result<Listing, SourceError> {
    let selection = &options.selection;
    let rows = Rows::of(options);
    let per_thread = rows != Rows::Processes && !selection.all && selection.depends_on_threads();
    let first = if per_thread {
        SnapshotRequest::default().with(FieldGroup::Threads)
    } else {
        SnapshotRequest::default()
    };
    let identities = source.snapshot(&first)?;
    let myself = Myself {
        euid: invocation.euid,
        terminal: find(&identities, invocation.myself)
            .and_then(|index| identities.processes[index].info.terminal)
            .map(|terminal| terminal.device),
    };
    let mut candidates: Vec<usize> = match selection.quick_pids() {
        Some(pids) => pids
            .iter()
            .filter_map(|&pid| find(&identities, Pid::from(pid)))
            .collect(),
        None => (0..identities.processes.len())
            .filter(|&index| identities.processes[index].info.pid.as_raw() != 0)
            .collect(),
    };

    if rows != Rows::Processes {
        candidates.dedup();
    }

    let candidates: Vec<usize> = candidates
        .into_iter()
        .filter(|&index| {
            let process = &identities.processes[index];
            let tasks = if per_thread {
                process.linux_tasks()
            } else {
                vec![process.linux_task()]
            };

            tasks.iter().any(|task| {
                let accepted = selection.accepts(&identities, task, myself, invocation.charset);

                selection.may_select(accepted)
            })
        })
        .collect();
    let request = data_request(options, per_thread);

    if request == SnapshotRequest::default() {
        return Ok(Listing {
            identities,
            data: None,
            candidates,
            myself,
        });
    }

    let mut data = source.snapshot(
        &request.for_processes(
            candidates
                .iter()
                .map(|&index| identities.processes[index].info.pid),
        ),
    )?;
    let candidates = candidates
        .into_iter()
        .filter_map(|index| {
            let identity = &identities.processes[index];
            let row = find(&data, identity.info.pid)?;
            let process = &mut data.processes[row];

            if process.info.start_time != identity.info.start_time {
                return None;
            }

            if per_thread && process.info.status != Status::Zombie {
                process.threads.clone_from(&identity.threads);
            }

            Some(row)
        })
        .collect();

    Ok(Listing {
        identities,
        data: Some(data),
        candidates,
        myself,
    })
}

/// Selects tasks from `listing` with the selection of `options`, by the
/// rules of procps-ng's `simple_spew` and `fancy_spew`. Each result pairs a
/// task with its `RowKind`. The caller applies the sort keys afterwards. With
/// rows for processes or for threads, the selection applies to the record
/// that each row shows, as in procps-ng.
///
/// With rows for processes and their threads, procps-ng sorts the tasks by
/// their process IDs. For each process, the first task that the selection
/// selects gives the row for the process. That task and every later task of
/// the process then give a row for a thread, without a further check of the
/// selection.
///
/// When that first task is not the main thread, procps-ng 4.0.7 prints the
/// process's row again and again and never finishes the listing. Its
/// `simple_spew` remembers the task's `ID_PID`, which is the thread's ID. The
/// loop of the thread rows starts at the same task and compares that ID with
/// the task's `ID_TGID`, which is the process ID, before it moves on. The two
/// differ, so the loop ends at once and the selection checks the same task
/// again. `ps` deliberately prints the process's row once and then the thread
/// rows, as procps-ng does when the first task is the main thread. procps-ng
/// never finishes such a listing, so only unit tests check this case.
fn select<'a>(
    listing: &'a Listing,
    options: &Options,
    charset: Charset,
    rows: Rows,
) -> Vec<(LinuxTask<'a>, RowKind)> {
    let data = listing.data();
    let selection = &options.selection;
    let wanted = |task: &LinuxTask<'_>| {
        let accepted = selection.accepts(&listing.identities, task, listing.myself, charset);

        selection.selects(accepted, task)
    };
    let processes = listing.candidates.iter().map(|&row| &data.processes[row]);

    match rows {
        Rows::Processes => processes
            .map(procps_core::Process::linux_task)
            .filter(|task| wanted(task))
            .map(|task| (task, RowKind::Process))
            .collect(),
        Rows::Threads => processes
            .flat_map(procps_core::Process::linux_tasks)
            .filter(|task| wanted(task))
            .map(|task| (task, RowKind::Thread))
            .collect(),
        Rows::ProcessesAndThreads => {
            let mut processes: Vec<_> = processes.collect();
            processes.sort_by_key(|process| process.info.pid);

            let tasks: Vec<LinuxTask<'a>> = processes
                .into_iter()
                .flat_map(procps_core::Process::linux_tasks)
                .collect();
            let mut selected = Vec::new();
            let mut index = 0;

            while index < tasks.len() {
                let task = tasks[index];

                if !wanted(&task) {
                    index += 1;
                    continue;
                }

                let pid = task.process().info.pid;
                selected.push((task, RowKind::Process));

                while let Some(&thread) = tasks.get(index)
                    && thread.process().info.pid == pid
                {
                    selected.push((thread, RowKind::Thread));
                    index += 1;
                }
            }

            selected
        }
    }
}

/// Lists the processes that `options` select, as procps-ng's `simple_spew`
/// and `fancy_spew` do, and returns whether `ps` printed a row. `system`
/// gives the names of users and groups.
///
/// # Errors
///
/// Returns an error if `options` need something that this project does not
/// implement yet, or if `source` cannot take a snapshot.
pub(crate) fn list(
    options: &Options,
    invocation: &Invocation,
    source: &impl ProcessSource,
    system: &dyn System,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<bool, PsError> {
    if let Some(unimplemented) = unimplemented(options) {
        return Err(PsError::Unimplemented(unimplemented));
    }

    // procps-ng's `fancy_spew` sorts and shows process trees.
    let fancy = options.forest.is_some() || !options.sort.is_empty();
    let quick = options.selection.quick_pids().is_some();
    let listing = read(options, invocation, source).map_err(|error| PsError::Source {
        quick,
        sorted: fancy,
        error,
    })?;

    if fancy && listing.identities.processes.is_empty() {
        return Err(PsError::EmptyProcessTable);
    }

    let data = listing.data();
    let mut rows = select(&listing, options, invocation.charset, Rows::of(options));
    let settings = print::Settings {
        caller: Caller::new(Uid::from(invocation.euid), Gid::from(invocation.egid)),
        charset: invocation.charset,
        modifiers: options.modifiers,
        date_format: options.date_format.clone(),
        page_size: invocation.page_size,
        calendar: invocation.calendar,
        forest: options.forest,
    };
    let mut context = Context::new(data, &listing.identities, settings, Names::new(system));

    let keys = match options.forest {
        Some(_) => forest::sort_keys(&options.sort),
        None => options.sort.clone(),
    };

    sort::sort(&mut rows, &keys, &mut context);

    let mut layout = Layout::new(
        &options.columns,
        layout::Settings {
            header: options.header,
            screen: options.screen,
            delimiter: options.delimiter,
            modifiers: options.modifiers,
        },
    );
    let both = options.threads.contains(ThreadFlags::SHOW_BOTH);
    let order: Vec<(usize, Vec<Branch>)> = if options.forest.is_some() {
        let processes: Vec<_> = rows
            .iter()
            .map(|(task, _)| (task.process().info.pid, task.process().info.ppid))
            .collect();

        forest::rows(&processes)
    } else {
        (0..rows.len()).map(|index| (index, Vec::new())).collect()
    };

    for (index, prefix) in order {
        let (task, kind) = &rows[index];

        context.set_forest_prefix(&prefix);
        layout.row(
            |specifier, room| {
                if both && kind.hides(specifier.print) {
                    return Cell::dash();
                }

                specifier.printer.print(&mut context, task, room)
            },
            stdout,
            stderr,
        );
    }

    Ok(layout.finish(stdout, stderr))
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        ffi::OsString,
        time::{Duration, SystemTime},
    };

    use assert_matches::assert_matches;
    use darwin_proc::{
        CalendarTime, Gid, Pid, RunState, SchedulingPolicy, Status, Terminal, ThreadInfo, Uid,
    };
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, FieldGroup, FixtureSource, Process, ProcessSource, Snapshot,
        SnapshotRequest, SourceError, Usage,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{Invocation, Rows, data_request, list, read, select};
    use crate::ps::{
        error::{PsError, Unimplemented},
        options::{Command, Modifiers, Options},
        parser,
        print::tests::{TAKEN, UPTIME, resources, snapshot, system, task},
    };

    /// The pid of `ps` itself in the fixtures.
    const MYSELF: i32 = 100;

    /// A process with `pid`, `parent`, `comm`, process group, session and
    /// owner, on the console when `terminal` is set. It sleeps, and the
    /// source read its usage and threads.
    fn fixture(
        pid: i32,
        parent: i32,
        comm: &str,
        group: i32,
        session: i32,
        uid: u32,
        terminal: bool,
    ) -> Process {
        let mut process = process(pid, comm);
        let info = &mut process.info;

        info.ppid = Pid::from(parent);
        info.pgid = Pid::from(group);
        info.session_leader = pid == session;
        info.credentials.ruid = Uid::from(uid);
        info.credentials.euid = Uid::from(uid);
        info.credentials.svuid = Uid::from(uid);
        info.terminal = terminal.then_some(Terminal {
            device: 0,
            foreground_group: Pid::from(60),
        });
        info.start_time = SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN - 100);
        process.session = Field::Available(Pid::from(session));
        process.arguments = Some(Field::Available(vec![OsString::from(comm)]));
        process.usage = Some(Usage {
            task: Field::Available(task()),
            resources: Field::Available(resources()),
            counters: Field::Unsupported,
        });
        process.threads = Some(Field::Available(vec![thread(1, b"", RunState::Waiting)]));

        process
    }

    /// The processes: `kernel_task`, `launchd`, a shell on the console with
    /// a job of two processes and `ps`, a daemon with root's real IDs and
    /// another user's effective IDs, and a zombie in the job's process group.
    /// The source could not read the zombie's session.
    fn fixtures() -> Vec<Process> {
        let mut cat = fixture(61, 50, "cat", 60, 50, 501, true);
        cat.threads = Some(Field::Available(vec![thread(1, b"", RunState::Running)]));
        let mut myself = fixture(MYSELF, 50, "ps", MYSELF, 50, 501, true);
        myself.threads = Some(Field::Available(vec![thread(1, b"", RunState::Running)]));
        let mut zombie = fixture(300, 50, "defunct", 60, 50, 501, false);
        let mut launchd = fixture(1, 0, "launchd", 1, 1, 0, false);
        launchd.info.credentials.rgid = Gid::from(0);
        launchd.info.credentials.egid = Gid::from(0);
        launchd.info.credentials.svgid = Gid::from(0);
        // The daemon runs a set-user-ID program that root started: its real
        // IDs are root's, and its effective and saved IDs are not.
        let mut daemon = fixture(200, 1, "daemon", 200, 200, 502, false);
        daemon.info.credentials.ruid = Uid::from(0);
        daemon.info.credentials.rgid = Gid::from(0);
        zombie.info.status = Status::Zombie;
        zombie.session = Field::Failed;

        vec![
            fixture(0, 0, "kernel_task", 0, 0, 0, false),
            launchd,
            fixture(50, 1, "zsh", 50, 50, 501, true),
            fixture(60, 50, "sleep", 60, 50, 501, true),
            cat,
            myself,
            daemon,
            zombie,
        ]
    }

    fn invocation() -> Invocation {
        Invocation {
            myself: Pid::from(MYSELF),
            euid: 501,
            egid: 20,
            charset: Charset::Utf8,
            page_size: 16_384,
            calendar: CalendarTime::utc,
        }
    }

    /// The options of the command line `line`.
    fn options(line: &[&str]) -> Options {
        let arguments: Vec<OsString> = ["ps"].iter().chain(line).map(OsString::from).collect();

        match parser::parse(&arguments, &system()).result {
            Ok(Command::Run(options)) => *options,
            other => panic!("{line:?} does not list processes: {other:?}"),
        }
    }

    /// A source whose snapshots contain `processes`, and which records the
    /// requests that it receives.
    struct Recording {
        fixtures: FixtureSource,
        requests: RefCell<Vec<SnapshotRequest>>,
    }

    impl Recording {
        fn new(processes: Vec<Process>) -> Self {
            Self {
                fixtures: FixtureSource::new(
                    SystemTime::UNIX_EPOCH + Duration::from_secs(TAKEN),
                    Duration::from_secs(UPTIME),
                    processes,
                )
                .with_system(snapshot(Vec::new()).system.expect("a system")),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl ProcessSource for Recording {
        fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
            self.requests.borrow_mut().push(request.clone());
            self.fixtures.snapshot(request)
        }
    }

    /// The pids of the rows that `options` select from `source`.
    fn selected_rows(options: &Options, source: &impl ProcessSource) -> Vec<i32> {
        let listing = read(options, &invocation(), source).expect("the fixtures have a snapshot");

        select(&listing, options, Charset::Utf8, Rows::of(options))
            .iter()
            .map(|(task, _)| task.process().info.pid.as_raw())
            .collect()
    }

    /// The pids that the command line `line` selects from `processes`.
    fn selected_from(line: &[&str], processes: Vec<Process>) -> Vec<i32> {
        selected_rows(&options(line), &Recording::new(processes))
    }

    /// The pids that the command line `line` selects from the fixtures.
    fn selected_pids(line: &[&str]) -> Vec<i32> {
        selected_from(line, fixtures())
    }

    #[rstest]
    #[case::default(&[], &[50, 60, 61, 100])]
    #[case::every_process(&["-A"], &[1, 50, 60, 61, 100, 200, 300])]
    #[case::every_process_e(&["-e"], &[1, 50, 60, 61, 100, 200, 300])]
    #[case::bsd_every_process(&["ax"], &[1, 50, 60, 61, 100, 200, 300])]
    #[case::terminal_without_leaders(&["-a"], &[60, 61, 100])]
    #[case::without_leaders(&["-d"], &[60, 61, 100, 300])]
    #[case::bsd_any_terminal(&["a"], &[50, 60, 61, 100])]
    #[case::bsd_own_user(&["x"], &[50, 60, 61, 100, 300])]
    #[case::negated_default(&["-N"], &[1, 200, 300])]
    #[case::negated_every_process(&["-N", "-e"], &[])]
    #[case::effective_user(&["-u", "502"], &[200])]
    #[case::effective_user_by_name(&["-u", "root"], &[1])]
    #[case::bsd_user(&["U", "502"], &[200])]
    #[case::real_user(&["-U", "0"], &[1, 200])]
    #[case::effective_root(&["-u", "0"], &[1])]
    #[case::real_group(&["-G", "staff"], &[50, 60, 61, 100, 300])]
    #[case::real_root_group(&["-G", "wheel"], &[1, 200])]
    #[case::effective_group(&["-g", "wheel"], &[1])]
    #[case::effective_staff(&["-g", "staff"], &[50, 60, 61, 100, 200, 300])]
    #[case::session(&["-s", "50"], &[50, 60, 61, 100, 300])]
    #[case::sessions_with_g(&["-g", "1,200"], &[1, 200])]
    #[case::trailing_session(&["+200"], &[200])]
    #[case::trailing_process_group(&["-60"], &[60, 61, 300])]
    #[case::terminal(&["-t", "console"], &[50, 60, 61, 100])]
    #[case::no_terminal(&["-t", "-"], &[1, 200, 300])]
    #[case::bsd_own_terminal(&["T"], &[50, 60, 61, 100])]
    #[case::commands(&["-C", "sleep,cat"], &[60, 61])]
    #[case::parents(&["--ppid", "50"], &[60, 61, 100, 300])]
    #[case::pids_in_list_order(&["-p", "61,60"], &[61, 60])]
    #[case::quick_pids_twice(&["-q", "60,60"], &[60, 60])]
    #[case::pids_with_another_list(&["-p", "61,60", "-u", "502"], &[60, 61, 200])]
    #[case::running(&["r", "-A"], &[61, 100])]
    #[case::running_quick(&["r", "-q", "60,61"], &[61])]
    #[case::lists_and_simple_selection(&["-a", "-u", "502"], &[60, 61, 100, 200])]
    #[case::kernel_task_never(&["-u", "0"], &[1])]
    #[case::long_user(&["--user", "502"], &[200])]
    #[case::long_real_user(&["--User", "0"], &[1, 200])]
    #[case::long_group(&["--group", "wheel"], &[1])]
    #[case::long_real_group(&["--Group", "20"], &[50, 60, 61, 100, 300])]
    #[case::long_pid(&["--pid", "61,60"], &[61, 60])]
    #[case::long_quick_pid(&["--quick-pid", "60"], &[60])]
    #[case::long_session(&["--sid", "200"], &[200])]
    #[case::long_terminal(&["--tty", "console"], &[50, 60, 61, 100])]
    #[case::long_deselect(&["--deselect", "-u", "501"], &[1, 200])]
    fn the_options_select_processes_as_procps_ng_selects_them(
        #[case] line: &[&str],
        #[case] expected: &[i32],
    ) {
        assert_eq!(selected_pids(line), expected);
    }

    #[test]
    fn a_long_command_name_matches_its_first_15_bytes() {
        let mut processes = fixtures();
        processes[6].info.comm = "a-very-long-daemon-name".into();

        assert_eq!(
            selected_from(&["-C", "a-very-long-daemon-name"], processes),
            [200]
        );
    }

    /// What `ps` writes on standard output and standard error for `line`,
    /// and its result.
    fn listed(line: &[&str]) -> (String, String, Result<bool, PsError>) {
        listed_from(line, fixtures())
    }

    /// What `ps` writes on standard output and standard error for `line`
    /// with the processes `processes`, and its result.
    fn listed_from(
        line: &[&str],
        processes: Vec<Process>,
    ) -> (String, String, Result<bool, PsError>) {
        let options = options(line);
        let source = Recording::new(processes);
        let system = system();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let result = list(
            &options,
            &invocation(),
            &source,
            &system,
            &mut stdout,
            &mut stderr,
        );

        (
            String::from_utf8(stdout).expect("the output is UTF-8"),
            String::from_utf8(stderr).expect("the output is UTF-8"),
            result,
        )
    }

    #[rstest]
    #[case::default(
        &[],
        "  PID TTY          TIME CMD\n   50 console  00:00:02 zsh\n   60 console  00:00:02 sleep\n   61 console  00:00:02 cat\n  100 console  00:00:02 ps\n"
    )]
    #[case::sorted(
        &["-o", "pid,user,comm", "--sort=-user,pid", "-A"],
        "  PID USER     COMMAND\n    1 root     launchd\n   50 laney    zsh\n   60 laney    sleep\n   61 laney    cat\n  100 laney    ps\n  300 laney    defunct <defunct>\n  200 502      daemon\n"
    )]
    #[case::bsd_user_format(
        &["u", "-p", "61"],
        "USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\nlaney       61  2.0  1.5 4194304 16384 console R+   11:58   0:02 cat\n"
    )]
    #[case::bsd_jobs_format(
        &["j", "-p", "60"],
        " PPID   PID  PGID   SID TTY      TPGID STAT   UID   TIME COMMAND\n   50    60    60    50 console     60 S+     501   0:02 sleep\n"
    )]
    #[case::full_format(
        &["-f", "-p", "50"],
        "UID        PID  PPID  C STIME TTY          TIME CMD\nlaney       50     1  2 11:58 console  00:00:02 zsh\n"
    )]
    #[case::long_format(
        &["-l", "-p", "50"],
        "F S   UID   PID  PPID  C PRI  NI ADDR SZ WCHAN  TTY          TIME CMD\n0 S   501    50     1  2  80   0 - 262144 -     console  00:00:02 zsh\n"
    )]
    #[case::session_of_a_listed_zombie(
        &["-o", "pid,sid", "-q", "300"],
        "  PID   SID\n  300    50\n"
    )]
    #[case::session_of_a_listed_zombie_with_its_usage(
        &["-o", "pid,sid,vsz", "-q", "300"],
        "  PID   SID    VSZ\n  300    50 4194304\n"
    )]
    #[case::numeric_long_format(
        &["nl", "-p", "50"],
        "F   UID   PID  PPID PRI  NI    VSZ   RSS  WCHAN STAT TTY        TIME COMMAND\n0   501    50     1  20   0 4194304 16384     - Ss   console    0:02 zsh\n"
    )]
    #[case::extra_full_format(
        &["-F", "-p", "50"],
        "UID        PID  PPID  C    SZ   RSS PSR STIME TTY          TIME CMD\nlaney       50     1  2 262144 16384  - 11:58 console  00:00:02 zsh\n"
    )]
    #[case::long_format_with_y(
        &["-ly", "-p", "50"],
        "S   UID   PID  PPID  C PRI  NI   RSS    SZ WCHAN  TTY          TIME CMD\nS   501    50     1  2  80   0 16384 262144 -     console  00:00:02 zsh\n"
    )]
    #[case::bsd_signal_format(
        &["s", "-p", "50"],
        "  UID   PID          PENDING          BLOCKED          IGNORED           CAUGHT STAT TTY        TIME COMMAND\n  501    50                -                - 0000000000000000 0000000000000000 Ss   console    0:02 zsh\n"
    )]
    #[case::bsd_memory_format(
        &["v", "-p", "50"],
        "  PID TTY      STAT   TIME  MAJFL   TRS   DRS   RSS %MEM COMMAND\n   50 console  Ss     0:02     10     -     - 16384  1.5 zsh\n"
    )]
    #[case::bsd_registers_format(
        &["X", "-p", "50"],
        "  PID           STACKP              ESP              EIP TMOUT ALARM STAT TTY        TIME COMMAND\n   50                - 0000000000000000 0000000000000000     -     - Ss   console    0:02 zsh\n"
    )]
    #[case::bsd_long_format(
        &["l", "-p", "50"],
        "F   UID   PID  PPID PRI  NI    VSZ   RSS WCHAN  STAT TTY        TIME COMMAND\n0   501    50     1  20   0 4194304 16384 -     Ss   console    0:02 zsh\n"
    )]
    fn ps_lists_the_selected_processes(#[case] line: &[&str], #[case] expected: &str) {
        assert_matches!(
            listed(line),
            (stdout, stderr, Ok(true)) if stdout == expected && stderr.is_empty(),
            "{}", listed(line).0
        );
    }

    #[test]
    fn without_a_selected_process_ps_prints_the_header_and_reports_it() {
        let (stdout, stderr, result) = listed(&["-p", "999"]);

        assert_eq!(
            (stdout.as_str(), stderr.as_str(), result.ok()),
            ("  PID TTY          TIME CMD\n", "", Some(false))
        );
    }

    // procps-ng's `fancy_spew` checks whether the process table that it
    // reaped is empty before it applies the selection, so an empty
    // selection still prints the header.
    #[test]
    fn a_sorted_listing_without_a_selected_process_prints_the_header() {
        let (stdout, stderr, result) = listed(&["--sort=pid", "-o", "pid,vsz", "-p", "999"]);

        assert_eq!(
            (stdout.as_str(), stderr.as_str(), result.ok()),
            ("  PID    VSZ\n", "", Some(false))
        );
    }

    /// The fixtures and a process with three threads, which has the pid
    /// 400 and is called `worker`. Its main thread has used a second of CPU
    /// time. Its second thread, called `io`, is running with a round-robin
    /// policy and has used two seconds.
    fn threaded_fixtures() -> Vec<Process> {
        let mut worker = fixture(400, 1, "worker", 400, 400, 501, false);
        worker.info.session_leader = false;
        worker.threads = Some(Field::Available(vec![
            ThreadInfo {
                user_time: Duration::from_secs(1),
                ..thread(401, b"", RunState::Waiting)
            },
            ThreadInfo {
                user_time: Duration::from_secs(2),
                policy: SchedulingPolicy::RoundRobin,
                base_priority: 47,
                ..thread(402, b"io", RunState::Running)
            },
            thread(403, b"", RunState::Waiting),
        ]));

        fixtures().into_iter().chain([worker]).collect()
    }

    #[rstest]
    #[case::unix_threads(
        &["-L", "-o", "pid,lwp,s,time,comm", "-p", "400"],
        "  PID   LWP S     TIME COMMAND\n  400   401 S 00:00:01 worker\n  400   402 R 00:00:02 io\n  400   403 S 00:00:00 worker\n"
    )]
    #[case::unix_threads_with_spid(
        &["-T", "-p", "400"],
        "  PID  SPID TTY          TIME CMD\n  400   401 ?        00:00:01 worker\n  400   402 ?        00:00:02 io\n  400   403 ?        00:00:00 worker\n"
    )]
    #[case::bsd_threads(
        &["H", "-o", "pid,stat,bsdtime,comm", "-p", "400"],
        "  PID STAT   TIME COMMAND\n  400 Sl     0:01 worker\n  400 Rl     0:02 io\n  400 Sl     0:00 worker\n"
    )]
    #[case::processes_and_threads(
        &["m", "-o", "pid,lwp,stat,time,comm", "-p", "400"],
        "  PID   LWP STAT     TIME COMMAND\n  400     - -    00:00:01 worker\n    -   401 Sl   00:00:01 -\n    -   402 Rl   00:00:02 -\n    -   403 Sl   00:00:00 -\n"
    )]
    #[case::unix_processes_and_threads(
        &["-m", "-o", "pid,tid,comm", "-q", "400,60"],
        "  PID   TID COMMAND\n   60     - sleep\n    -     1 -\n  400     - worker\n    -   401 -\n    -   402 -\n    -   403 -\n"
    )]
    #[case::thread_by_name(&["-L", "-C", "io", "-o", "pid,lwp,comm"], "  PID   LWP COMMAND\n  400   402 io\n")]
    #[case::thread_by_name_with_its_time(&["-L", "-C", "io", "-o", "lwp,s,time"], "  LWP S     TIME\n  402 R 00:00:02\n")]
    // procps-ng 4.0.7 crashes when a pid that does not exist separates two
    // copies of a pid. `ps` lists the pid's threads once.
    #[case::quick_pid_after_a_missing_pid(&["-L", "-o", "lwp", "-q", "400,999,400"], "  LWP\n  401\n  402\n  403\n")]
    // procps-ng 4.0.7 prints the process's row for ever when the first task
    // that the selection selects is not the main thread. `ps` prints the row
    // once, and then the rows of that thread and of the later threads.
    #[case::process_whose_main_thread_is_not_selected(
        &["m", "-C", "io", "-o", "pid,lwp,comm"],
        "  PID   LWP COMMAND\n  400     - io\n    -   402 -\n    -   403 -\n"
    )]
    // procps-ng reads the threads of a pid in the list of `-q` only when the
    // previous pid that exists is a different process.
    #[case::quick_pid_after_itself(&["-L", "-o", "lwp", "-q", "400,400"], "  LWP\n  401\n  402\n  403\n")]
    #[case::quick_pid_after_another(
        &["-L", "-o", "lwp", "-q", "400,60,400"],
        "  LWP\n  401\n  402\n  403\n    1\n  401\n  402\n  403\n"
    )]
    #[case::processes_and_threads_of_a_quick_pid_after_itself(
        &["-m", "-o", "pid,tid", "-q", "400,400"],
        "  PID   TID\n  400     -\n    -   401\n    -   402\n    -   403\n"
    )]
    #[case::running_threads(&["r", "-L", "-o", "lwp,s", "-p", "400"], "  LWP S\n  402 R\n")]
    #[case::per_thread_values(
        &["-L", "-o", "lwp,maj_flt,min_flt,pri,cls,rtprio", "-p", "400"],
        "  LWP  MAJFL  MINFL PRI CLS RTPRIO\n  401      -      -  19  TS      -\n  402      -      -  87  RR     47\n  403      -      -  19  TS      -\n"
    )]
    #[case::sorted_unix_threads(&["-L", "--sort=pid", "-o", "pid,lwp", "-p", "400"], "  PID   LWP\n  400   400\n")]
    #[case::sorted_processes_and_threads(
        &["m", "--sort=pid", "-o", "pid,lwp,comm", "-p", "400"],
        "  PID   LWP COMMAND\n  400     - worker\n"
    )]
    #[case::sorted_bsd_threads(
        &["H", "--sort=-lwp", "-o", "pid,lwp,comm", "-p", "400"],
        "  PID   LWP COMMAND\n  400   403 worker\n  400   402 io\n  400   401 worker\n"
    )]
    fn ps_lists_threads_as_procps_ng_lists_them(#[case] line: &[&str], #[case] expected: &str) {
        let (stdout, stderr, result) = listed_from(line, threaded_fixtures());

        assert_eq!(
            (stdout.as_str(), stderr.as_str(), result.ok()),
            (expected, "", Some(true))
        );
    }

    /// A source like `Recording`, whose process 400 has become a zombie by
    /// the second snapshot. Darwin keeps no threads for a zombie.
    struct ZombieAfterFirst {
        source: Recording,
        snapshots: RefCell<usize>,
    }

    impl ProcessSource for ZombieAfterFirst {
        fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
            let mut snapshot = self.source.snapshot(request)?;
            let mut snapshots = self.snapshots.borrow_mut();

            if *snapshots > 0 {
                for process in snapshot
                    .processes
                    .iter_mut()
                    .filter(|process| process.info.pid == Pid::from(400))
                {
                    process.info.status = Status::Zombie;
                    process.threads = request
                        .wants(FieldGroup::Threads)
                        .then_some(Field::Unsupported);
                }
            }

            *snapshots += 1;
            Ok(snapshot)
        }
    }

    // A zombie keeps its start time, so it stays a candidate. Its row comes
    // from the second snapshot, which has no threads for it, so it is one
    // task with the process's name, as a zombie is on Linux.
    #[rstest]
    #[case::thread_name(&["-L", "-C", "io", "-o", "lwp,s,nlwp,rss"], "  LWP S NLWP   RSS\n", false)]
    #[case::process_name(
        &["-L", "-C", "worker,io", "-o", "lwp,s,nlwp,rss"],
        "  LWP S NLWP   RSS\n  400 Z    1 16384\n",
        true
    )]
    fn a_candidate_that_becomes_a_zombie_has_no_threads(
        #[case] line: &[&str],
        #[case] expected: &str,
        #[case] listed: bool,
    ) {
        let source = ZombieAfterFirst {
            source: Recording::new(threaded_fixtures()),
            snapshots: RefCell::new(0),
        };
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let result = list(
            &options(line),
            &invocation(),
            &source,
            &system(),
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(
            (
                String::from_utf8(stdout).expect("the output is UTF-8"),
                String::from_utf8(stderr).expect("the output is UTF-8"),
                result.ok()
            ),
            (expected.to_owned(), String::new(), Some(listed))
        );
    }

    #[rstest]
    #[case::unix_threads(&["-L", "-o", "pid"])]
    #[case::bsd_threads(&["H", "-o", "pid"])]
    #[case::processes_and_threads(&["m", "-o", "pid"])]
    fn rows_of_threads_need_the_threads(#[case] line: &[&str]) {
        assert_eq!(
            data_request(&options(line), false),
            SnapshotRequest::default().with(FieldGroup::Threads)
        );
    }

    // procps-ng's `fancy_spew` reads the threads only for BSD's `H`.
    #[test]
    fn a_sorted_listing_of_processes_does_not_read_the_threads() {
        assert_eq!(
            data_request(&options(&["-L", "--sort=pid", "-o", "pid"]), false),
            SnapshotRequest::default()
        );
    }

    /// A source that cannot take a snapshot.
    struct Failing;

    impl ProcessSource for Failing {
        fn snapshot(&self, _: &SnapshotRequest) -> Result<Snapshot, SourceError> {
            Err(SourceError::TimedOut)
        }
    }

    /// The error that `list` returns for `line` with `source`.
    fn error_of(line: &[&str], source: &impl ProcessSource) -> Option<PsError> {
        list(
            &options(line),
            &invocation(),
            source,
            &system(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .err()
    }

    // procps-ng sorts and shows process trees in `fancy_spew`, which reports
    // its own errors.
    #[rstest]
    #[case::simple(&["-o", "pid"], false)]
    #[case::sorted(&["--sort=pid", "-o", "pid"], true)]
    #[case::forest(&["--forest", "-o", "pid"], true)]
    fn a_failed_snapshot_reports_the_listing_s_error(#[case] line: &[&str], #[case] fancy: bool) {
        assert_matches!(
            error_of(line, &Failing),
            Some(PsError::Source { quick: false, sorted, error: SourceError::TimedOut })
                if sorted == fancy
        );
    }

    #[rstest]
    #[case::sorted(&["--sort=pid", "-o", "pid"])]
    #[case::forest(&["--forest", "-o", "pid"])]
    fn a_fancy_listing_of_an_empty_table_is_an_error(#[case] line: &[&str]) {
        assert_matches!(
            error_of(line, &Recording::new(Vec::new())),
            Some(PsError::EmptyProcessTable)
        );
    }

    #[rstest]
    #[case::gnu(
        &["--forest", "-A", "-o", "pid,comm"],
        "  PID COMMAND\n    1 launchd\n   50  \\_ zsh\n   60  |   \\_ sleep\n   61  |   \\_ cat\n  100  |   \\_ ps\n  300  |   \\_ defunct <defunct>\n  200  \\_ daemon\n"
    )]
    #[case::bsd(
        &["f", "-A", "-o", "pid,args"],
        "  PID COMMAND\n    1 launchd\n   50  \\_ zsh\n   60  |   \\_ sleep\n   61  |   \\_ cat\n  100  |   \\_ ps\n  300  |   \\_ defunct\n  200  \\_ daemon\n"
    )]
    #[case::unix(
        &["-H", "-A", "-o", "pid,fname"],
        "  PID COMMAND\n    1 launchd\n   50   zsh\n   60     sleep\n   61     cat\n  100     ps\n  300     defunct\n  200   daemon\n"
    )]
    #[case::sorted_siblings(
        &["--forest", "--sort=-pid", "-A", "-o", "pid,comm"],
        "  PID COMMAND\n    1 launchd\n  200  \\_ daemon\n   50  \\_ zsh\n  300      \\_ defunct <defunct>\n  100      \\_ ps\n   61      \\_ cat\n   60      \\_ sleep\n"
    )]
    #[case::cut_prefix(
        &["--forest", "-A", "-o", "comm:6,pid"],
        "COMMAND  PID\nlaunch     1\n \\_ zs    50\n |  sl    60\n |  ca    61\n |  ps   100\n |  de   300\n \\_ da   200\n"
    )]
    #[case::subtree(
        &["--forest", "--ppid", "50", "-o", "pid,comm"],
        "  PID COMMAND\n  300 defunct <defunct>\n  100 ps\n   61 cat\n   60 sleep\n"
    )]
    fn ps_shows_process_trees_as_procps_ng_shows_them(
        #[case] line: &[&str],
        #[case] expected: &str,
    ) {
        let (stdout, stderr, result) = listed(line);

        assert_eq!(
            (stdout.as_str(), stderr.as_str(), result.ok()),
            (expected, "", Some(true))
        );
    }

    #[rstest]
    #[case::environment(&["e"], Unimplemented::Option("e"))]
    #[case::cumulative(&["S"], Unimplemented::Option("S"))]
    #[case::signal_names(&["--signames"], Unimplemented::Option("--signames"))]
    fn a_part_that_is_not_implemented_yet_is_an_error(
        #[case] line: &[&str],
        #[case] expected: Unimplemented,
    ) {
        assert_matches!(
            listed(line),
            (stdout, _, Err(PsError::Unimplemented(unimplemented)))
                if stdout.is_empty() && unimplemented == expected
        );
    }

    /// The request for the identity and the usage group.
    fn usage() -> SnapshotRequest {
        SnapshotRequest::default().with(FieldGroup::Usage)
    }

    #[rstest]
    #[case::identity(&["-o", "pid,comm"], SnapshotRequest::default())]
    #[case::command_line(&["-o", "args"], SnapshotRequest::default().with(FieldGroup::Arguments))]
    #[case::command_names(&["c", "-o", "args"], SnapshotRequest::default())]
    #[case::state(&["-o", "stat"], SnapshotRequest::default().with(FieldGroup::Threads))]
    #[case::memory(
        &["-o", "%mem"],
        SnapshotRequest::default().with(FieldGroup::Usage).with_system()
    )]
    #[case::regions(
        &["-o", "trs"],
        SnapshotRequest::default().with(FieldGroup::Usage).with(FieldGroup::Regions)
    )]
    #[case::sort_key(&["-o", "pid", "--sort", "cmd"], SnapshotRequest::default().with(FieldGroup::Arguments))]
    #[case::running(&["r", "-o", "pid"], SnapshotRequest::default().with(FieldGroup::Threads))]
    #[case::environment(&["-o", "environ"], SnapshotRequest::default().with(FieldGroup::Environment))]
    #[case::descriptors(
        &["-o", "fds"],
        SnapshotRequest::default().with(FieldGroup::FileDescriptors)
    )]
    #[case::unique(
        &["-o", "uss"],
        SnapshotRequest::default().with(FieldGroup::Usage).with(FieldGroup::Regions)
    )]
    #[case::identity_values(&["-o", "exe,supgid,supgrp,lim"], SnapshotRequest::default())]
    #[case::read_bytes(&["-o", "rbytes"], SnapshotRequest::default().with(FieldGroup::Usage))]
    #[case::written_bytes(&["-o", "wbytes"], SnapshotRequest::default().with(FieldGroup::Usage))]
    #[case::cpu_use_with_children(&["-o", "cuc"], SnapshotRequest::default().with(FieldGroup::Usage))]
    #[case::sort_by_descriptors(
        &["-o", "pid", "--sort", "fds"],
        SnapshotRequest::default().with(FieldGroup::FileDescriptors)
    )]
    #[case::sort_by_unique(
        &["-o", "pid", "--sort", "uss"],
        SnapshotRequest::default().with(FieldGroup::Usage).with(FieldGroup::Regions)
    )]
    #[case::sort_by_code(
        &["-o", "pid", "--sort", "vm_exe"],
        SnapshotRequest::default().with(FieldGroup::Usage).with(FieldGroup::Regions)
    )]
    #[case::sort_by_read_bytes(&["-o", "pid", "--sort", "rbytes"], usage())]
    #[case::sort_by_written_bytes(&["-o", "pid", "--sort", "wbytes"], usage())]
    #[case::sort_by_children_user_time(&["-o", "pid", "--sort", "cutime"], usage())]
    #[case::sort_by_cpu_use_with_children(&["-o", "pid", "--sort", "cuc"], usage())]
    #[case::sort_by_resident_pages(&["-o", "pid", "--sort", "resident"], usage())]
    #[case::sort_by_wired(&["-o", "pid", "--sort", "vm_lock"], usage())]
    #[case::darwin_footprint(&["-o", "footprint"], usage())]
    #[case::sort_by_darwin_footprint(&["-o", "pid", "--sort", "footprint"], usage())]
    #[case::darwin_max_footprint(&["-o", "maxfootprint"], usage())]
    #[case::sort_by_darwin_max_footprint(&["-o", "pid", "--sort", "maxfootprint"], usage())]
    #[case::darwin_wired(&["-o", "wired"], usage())]
    #[case::sort_by_darwin_wired(&["-o", "pid", "--sort", "wired"], usage())]
    #[case::darwin_context_switches(&["-o", "csw"], usage())]
    #[case::sort_by_darwin_context_switches(&["-o", "pid", "--sort", "csw"], usage())]
    #[case::darwin_instructions(&["-o", "instrs"], usage())]
    #[case::sort_by_darwin_instructions(&["-o", "pid", "--sort", "instrs"], usage())]
    #[case::darwin_cycles(&["-o", "cycles"], usage())]
    #[case::sort_by_darwin_cycles(&["-o", "pid", "--sort", "cycles"], usage())]
    #[case::darwin_energy(&["-o", "energy"], usage())]
    #[case::sort_by_darwin_energy(&["-o", "pid", "--sort", "energy"], usage())]
    #[case::darwin_architecture(&["-o", "arch", "--sort", "arch"], SnapshotRequest::default())]
    fn the_request_asks_only_for_what_the_options_need(
        #[case] line: &[&str],
        #[case] expected: SnapshotRequest,
    ) {
        assert_eq!(data_request(&options(line), false), expected);
    }

    fn for_pids(request: SnapshotRequest, pids: &[i32]) -> SnapshotRequest {
        request.for_processes(pids.iter().map(|&pid| Pid::from(pid)))
    }

    #[rstest]
    #[case::identity_only(&["--ppid", "50", "-o", "pid,comm"], vec![])]
    #[case::regions_of_the_selected_processes(
        &["--ppid", "50", "-o", "pid,trs"],
        vec![for_pids(
            SnapshotRequest::default().with(FieldGroup::Usage).with(FieldGroup::Regions),
            &[60, 61, 100, 300]
        )]
    )]
    #[case::quick_pids(
        &["-q", "61,60", "-o", "pid,vsz"],
        vec![for_pids(SnapshotRequest::default().with(FieldGroup::Usage), &[60, 61])]
    )]
    #[case::running(
        &["r", "-u", "502", "-o", "pid"],
        vec![for_pids(SnapshotRequest::default().with(FieldGroup::Threads), &[200])]
    )]
    #[case::command_name_without_thread_rows(
        &["-C", "sleep", "-o", "pid,trs"],
        vec![for_pids(
            SnapshotRequest::default().with(FieldGroup::Usage).with(FieldGroup::Regions),
            &[60]
        )]
    )]
    #[case::running_negated(
        &["r", "-N", "-u", "502", "-o", "pid"],
        vec![for_pids(
            SnapshotRequest::default().with(FieldGroup::Threads),
            &[1, 50, 60, 61, 100, 200, 300]
        )]
    )]
    fn the_data_is_read_only_for_the_processes_that_may_be_selected(
        #[case] line: &[&str],
        #[case] data: Vec<SnapshotRequest>,
    ) {
        let source = Recording::new(fixtures());
        let expected: Vec<SnapshotRequest> = [SnapshotRequest::default()]
            .into_iter()
            .chain(data)
            .collect();

        read(&options(line), &invocation(), &source).expect("the fixtures have a snapshot");

        assert_eq!(source.requests.into_inner(), expected);
    }

    /// The request for the threads group and `groups`.
    fn threads_and(groups: &[FieldGroup]) -> SnapshotRequest {
        groups.iter().fold(
            SnapshotRequest::default().with(FieldGroup::Threads),
            |request, &group| request.with(group),
        )
    }

    /// The request for `groups`.
    fn only(groups: &[FieldGroup]) -> SnapshotRequest {
        groups
            .iter()
            .fold(SnapshotRequest::default(), |request, &group| {
                request.with(group)
            })
    }

    // The names of threads decide which processes a selection by command
    // name may select, so the first snapshot reads the threads. The second
    // reads the other data only for the processes that have a selected task,
    // and does not read the threads again. `-A` accepts every task whatever
    // its name, and a listing without `-C` needs no names to choose its
    // candidates.
    #[rstest]
    #[case::thread_name(
        &["-L", "-C", "io", "-o", "pid,lwp,trs"],
        vec![threads_and(&[]), for_pids(only(&[FieldGroup::Usage, FieldGroup::Regions]), &[400])]
    )]
    #[case::main_thread_name(
        &["H", "-C", "sleep,worker", "-o", "pid,trs"],
        vec![threads_and(&[]), for_pids(only(&[FieldGroup::Usage, FieldGroup::Regions]), &[60, 400])]
    )]
    #[case::negated(
        &["-L", "-N", "-C", "worker", "-o", "pid,trs"],
        vec![
            threads_and(&[]),
            for_pids(
                only(&[FieldGroup::Usage, FieldGroup::Regions]),
                &[1, 50, 60, 61, 100, 200, 300, 400]
            )
        ]
    )]
    #[case::threads_only(&["-L", "-C", "io", "-o", "pid,lwp"], vec![threads_and(&[])])]
    #[case::every_process(
        &["-L", "-A", "-C", "io", "-o", "pid,lwp"],
        vec![SnapshotRequest::default(), for_pids(threads_and(&[]), &[1, 50, 60, 61, 100, 200, 300, 400])]
    )]
    #[case::without_command_names(
        &["-L", "-o", "pid,lwp,trs"],
        vec![
            SnapshotRequest::default(),
            for_pids(threads_and(&[FieldGroup::Usage, FieldGroup::Regions]), &[50, 60, 61, 100])
        ]
    )]
    fn a_selection_by_thread_name_reads_the_threads_first(
        #[case] line: &[&str],
        #[case] expected: Vec<SnapshotRequest>,
    ) {
        let source = Recording::new(threaded_fixtures());

        read(&options(line), &invocation(), &source).expect("the fixtures have a snapshot");

        assert_eq!(source.requests.into_inner(), expected);
    }

    /// A source like `Recording`, which applies `change` to the process
    /// `pid` in each snapshot that asks for more than the identity, as if
    /// the process changed between the two snapshots of `read`.
    struct Changing {
        source: Recording,
        pid: Pid,
        change: fn(&mut Process),
    }

    impl ProcessSource for Changing {
        fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError> {
            let mut snapshot = self.source.snapshot(request)?;

            if request != &SnapshotRequest::default() {
                snapshot
                    .processes
                    .iter_mut()
                    .filter(|process| process.info.pid == self.pid)
                    .for_each(self.change);
            }

            Ok(snapshot)
        }
    }

    /// The pids that the command line `line` selects from the fixtures when
    /// `change` modifies the process `pid` between the identity snapshot and
    /// the data snapshot.
    fn selected_after_change(line: &[&str], pid: i32, change: fn(&mut Process)) -> Vec<i32> {
        let source = Changing {
            source: Recording::new(fixtures()),
            pid: Pid::from(pid),
            change,
        };

        selected_rows(&options(line), &source)
    }

    #[test]
    fn a_process_that_another_process_replaced_is_left_out() {
        let pids = selected_after_change(&["--ppid", "50", "-o", "vsz"], 60, |process| {
            process.info.start_time += Duration::from_secs(1);
        });

        assert_eq!(pids, [61, 100, 300]);
    }

    // procps-ng prints each row from the record that its selection read, so
    // a row always satisfies the selection.
    #[rstest]
    #[case::effective_user(
        &["-u", "502", "-o", "pid,vsz"],
        200,
        |process: &mut Process| process.info.credentials.euid = Uid::from(503),
        &[]
    )]
    #[case::session(
        &["-s", "200", "-o", "pid,vsz"],
        200,
        |process: &mut Process| process.session = Field::Available(Pid::from(201)),
        &[]
    )]
    #[case::parent(
        &["--ppid", "1", "-o", "pid,vsz"],
        200,
        |process: &mut Process| process.info.ppid = Pid::from(300),
        &[50]
    )]
    #[case::exited_group_leader(
        &["-s", "200", "-o", "pid,vsz"],
        200,
        |process: &mut Process| {
            process.info.status = Status::Zombie;
            process.session = Field::Failed;
        },
        &[200]
    )]
    #[case::negated(
        &["-N", "-u", "502", "-o", "pid,vsz"],
        1,
        |process: &mut Process| process.info.credentials.euid = Uid::from(502),
        &[50, 60, 61, 100, 300]
    )]
    fn a_process_that_changed_is_selected_by_its_new_values(
        #[case] line: &[&str],
        #[case] pid: i32,
        #[case] change: fn(&mut Process),
        #[case] expected: &[i32],
    ) {
        assert_eq!(selected_after_change(line, pid, change), expected);
    }

    // procps-ng's `-f` shows the command line in the `comm` column of a
    // personality's format, such as the format of `-f` with the `debian`
    // personality.
    #[test]
    fn the_full_format_reads_the_command_line_for_comm() {
        let mut options = options(&["-o", "comm"]);
        options.modifiers |= Modifiers::FULL;

        assert_eq!(
            data_request(&options, false),
            SnapshotRequest::default().with(FieldGroup::Arguments)
        );
    }
}
