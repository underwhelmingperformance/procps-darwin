// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/display.c.

use std::{collections::BTreeSet, io::Write};

use darwin_proc::{CalendarTime, Gid, Pid, Uid};
use procps_core::{
    Charset, LinuxTask, ProcessSource, Snapshot, SnapshotRequest, SourceError, helper::Caller,
};

use crate::ps::{
    error::{PsError, Unimplemented},
    format::{Content, Order},
    layout::{self, Layout},
    names::Names,
    options::{Modifiers, Options, ThreadFlags},
    print::{self, Context},
    selection::Myself,
    sort,
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
    if options.threads != ThreadFlags::SHOW_PROCESS {
        return Some(Unimplemented::Threads);
    }

    if options.forest.is_some() {
        return Some(Unimplemented::Forest);
    }

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

/// The request for the data that `options` show: the field groups that the
/// columns, the sort keys and the selection need, and the system statistics
/// for `%mem`. `ps` asks for the data only for the processes that the
/// selection may select.
pub(crate) fn data_request(options: &Options) -> SnapshotRequest {
    let printers = options
        .columns
        .iter()
        .filter_map(|column| match column.content {
            Content::Specifier(specifier) => Some(specifier.printer),
            Content::Text => None,
        });
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
        .copied()
        .collect();
    let request = groups
        .into_iter()
        .fold(SnapshotRequest::default(), SnapshotRequest::with);

    if printers.clone().any(print::Printer::needs_system) {
        return request.with_system();
    }

    request
}

/// The processes that a listing shows.
#[derive(Debug)]
struct Listing {
    /// The identity of every process. The selection and the session columns
    /// read it.
    identities: Snapshot,
    /// The data of the processes that the selection may select, or `None` if
    /// the columns, the sort keys and the selection need only the identity.
    data: Option<Snapshot>,
    /// The selected processes, as indices into the data's processes, in the
    /// order that procps-ng reads them.
    rows: Vec<usize>,
}

impl Listing {
    /// The snapshot that the rows index.
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

/// Reads the processes that `options` select from `source`. The first
/// snapshot has the identity of every process. If the columns, the sort
/// keys or the selection need more than the identity, a second snapshot
/// reads that data. It reads only the processes that the selection may
/// select according to their identities, because a walk of the memory
/// regions of every process takes seconds. The selection is then applied
/// again to the second snapshot's record of each process, which is the
/// record that the row shows, so every row satisfies the selection. A
/// process that exits between the two snapshots is left out, and so is one
/// whose pid another process has taken by then.
///
/// The rows are in the order that procps-ng reads the processes: the order
/// of the list for a selection that reads its processes directly, such as
/// `-q` or a `-p` on its own, and otherwise ascending order of pid, as
/// Linux's `/proc` lists them. Linux does not show its idle tasks, which have
/// pid 0, in `/proc`, so procps-ng never lists a process with pid 0. `ps`
/// leaves out `kernel_task`, which has pid 0 on Darwin.
fn read(
    options: &Options,
    invocation: &Invocation,
    source: &impl ProcessSource,
) -> Result<Listing, SourceError> {
    let selection = &options.selection;
    let identities = source.snapshot(&SnapshotRequest::default())?;
    let myself = Myself {
        euid: invocation.euid,
        terminal: find(&identities, invocation.myself)
            .and_then(|index| identities.processes[index].info.terminal)
            .map(|terminal| terminal.device),
    };
    let candidates: Vec<usize> = match selection.quick_pids() {
        Some(pids) => pids
            .iter()
            .filter_map(|&pid| find(&identities, Pid::from(pid)))
            .collect(),
        None => (0..identities.processes.len())
            .filter(|&index| identities.processes[index].info.pid.as_raw() != 0)
            .collect(),
    };
    let accepted: Vec<(usize, bool)> = candidates
        .into_iter()
        .map(|index| {
            let process = &identities.processes[index];

            (
                index,
                selection.accepts(&identities, process, myself, invocation.charset),
            )
        })
        .filter(|&(_, accepted)| selection.may_select(accepted))
        .collect();
    let request = data_request(options);

    if request == SnapshotRequest::default() {
        let rows = accepted
            .into_iter()
            .filter(|&(index, accepted)| selection.selects(accepted, &identities.processes[index]))
            .map(|(index, _)| index)
            .collect();

        return Ok(Listing {
            identities,
            data: None,
            rows,
        });
    }

    let data = source.snapshot(
        &request.for_processes(
            accepted
                .iter()
                .map(|&(index, _)| identities.processes[index].info.pid),
        ),
    )?;
    let rows = accepted
        .into_iter()
        .filter_map(|(index, _)| {
            let identity = &identities.processes[index].info;
            let row = find(&data, identity.pid)?;
            let process = &data.processes[row];

            if process.info.start_time != identity.start_time {
                return None;
            }

            let accepted = selection.accepts(&identities, process, myself, invocation.charset);

            selection.selects(accepted, process).then_some(row)
        })
        .collect();

    Ok(Listing {
        identities,
        data: Some(data),
        rows,
    })
}

/// Lists the processes that `options` select, as procps-ng's `simple_spew`
/// and `fancy_spew` do, and returns whether `ps` printed a row for a
/// process. `system` gives the names of users and groups.
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

    let sorted = !options.sort.is_empty();
    let quick = options.selection.quick_pids().is_some();
    let listing = read(options, invocation, source).map_err(|error| PsError::Source {
        quick,
        sorted,
        error,
    })?;

    if sorted && listing.identities.processes.is_empty() {
        return Err(PsError::EmptyProcessTable);
    }

    let data = listing.data();
    let mut tasks: Vec<LinuxTask<'_>> = listing
        .rows
        .iter()
        .map(|&row| data.processes[row].linux_task())
        .collect();
    let settings = print::Settings {
        caller: Caller::new(Uid::from(invocation.euid), Gid::from(invocation.egid)),
        charset: invocation.charset,
        modifiers: options.modifiers,
        date_format: options.date_format.clone(),
        page_size: invocation.page_size,
        calendar: invocation.calendar,
    };
    let mut context = Context::new(data, &listing.identities, settings, Names::new(system));

    sort::sort(&mut tasks, &options.sort, &mut context);

    let mut layout = Layout::new(
        &options.columns,
        layout::Settings {
            header: options.header,
            screen: options.screen,
            delimiter: options.delimiter,
            modifiers: options.modifiers,
        },
    );

    for task in &tasks {
        layout.row(
            |specifier, room| specifier.printer.print(&mut context, task, room),
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
    use darwin_proc::{CalendarTime, Gid, Pid, RunState, Status, Terminal, Uid};
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, FieldGroup, FixtureSource, Process, ProcessSource, Snapshot,
        SnapshotRequest, SourceError, Usage,
        testing::{process, thread},
    };
    use rstest::rstest;

    use super::{Invocation, data_request, list, read};
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

    /// The pids that the command line `line` selects from `processes`.
    fn selected_from(line: &[&str], processes: Vec<Process>) -> Vec<i32> {
        let listing = read(&options(line), &invocation(), &Recording::new(processes))
            .expect("the fixtures have a snapshot");
        let data = listing.data();

        listing
            .rows
            .iter()
            .map(|&row| data.processes[row].info.pid.as_raw())
            .collect()
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
        let options = options(line);
        let source = Recording::new(fixtures());
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

    #[rstest]
    #[case::threads(&["-L"], Unimplemented::Threads)]
    #[case::forest(&["--forest"], Unimplemented::Forest)]
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
    fn the_request_asks_only_for_what_the_options_need(
        #[case] line: &[&str],
        #[case] expected: SnapshotRequest,
    ) {
        assert_eq!(data_request(&options(line)), expected);
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
        let listing =
            read(&options(line), &invocation(), &source).expect("the fixtures have a snapshot");
        let data = listing.data();

        listing
            .rows
            .iter()
            .map(|&row| data.processes[row].info.pid.as_raw())
            .collect()
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
            data_request(&options),
            SnapshotRequest::default().with(FieldGroup::Arguments)
        );
    }
}
