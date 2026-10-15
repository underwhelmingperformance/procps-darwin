// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2009-2026 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2013-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 2012 Roberto Polli <rpolli@babel.it>
// SPDX-FileCopyrightText: 2002-2007 Albert Cahalan
// SPDX-FileCopyrightText: 2000 Kjetil Torgrim Homme <kjetilho@ifi.uio.no>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/pgrep.c.

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
};

use darwin_proc::{Pid, Regex};
use procps_core::{COMMAND_NAME_LENGTH, Charset, FieldGroup, LinuxTask, Snapshot, SnapshotRequest};

use crate::{
    linux::{Clock, session, terminal_name},
    pgrep::{
        Mode, Options, PgrepError,
        options::{Listing, Pick, Tasks},
    },
};

/// What the selection and the report need to know about the running tool.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Invocation<'a> {
    /// Which tool runs.
    pub(crate) mode: Mode,
    /// The name that the tool was run as, without its directory.
    pub(crate) program: &'a OsStr,
    /// The tool's own process, which is never selected.
    pub(crate) myself: Pid,
    /// The locale's character encoding, which decides how names and command
    /// lines are escaped before they are matched and printed.
    pub(crate) charset: Charset,
}

/// The snapshot request for `options`. It covers every process, and adds the
/// arguments for `-f` and `-a`, the environment for `--env`, and the threads
/// for `-w` and `-r`. `-r` needs the threads because they decide the state.
pub(crate) fn request(options: &Options) -> SnapshotRequest {
    let mut request = SnapshotRequest::default();

    if options.matching.full || options.output.listing == Listing::CommandLines {
        request = request.with(FieldGroup::Arguments);
    }

    if options.filters.environment.is_some() {
        request = request.with(FieldGroup::Environment);
    }

    if options.filters.states.is_some() || options.tasks == Tasks::Threads {
        request = request.with(FieldGroup::Threads);
    }

    request
}

/// The options of a selection, with the pattern compiled.
#[derive(Debug)]
pub(crate) struct Selection<'a> {
    options: &'a Options,
    regex: Option<Regex>,
}

impl<'a> Selection<'a> {
    /// The selection that `options` describes. procps-ng compiles the pattern
    /// when it starts to select processes, so a regex error comes before an
    /// error in reading them. The exception is `-A`, for which procps-ng reads
    /// the processes while it parses the options. After compiling the pattern,
    /// the tools refuse an option for a Linux concept. procps-ng would go on to
    /// select processes at that point.
    pub(crate) fn new(options: &'a Options) -> Result<Self, PgrepError> {
        let regex = compile(options)?;

        if let Some(option) = options.unsupported {
            return Err(PgrepError::Unsupported(option));
        }

        Ok(Self { options, regex })
    }

    /// The options of the selection.
    pub(crate) const fn options(&self) -> &Options {
        self.options
    }
}

/// The pattern as procps-ng's `do_regcomp` compiles it, or `None` without a
/// pattern.
fn compile(options: &Options) -> Result<Option<Regex>, PgrepError> {
    let Some(pattern) = &options.pattern else {
        return Ok(None);
    };

    let pattern = if options.matching.exact {
        OsString::from_vec([b"^(", pattern.as_bytes(), b")$"].concat())
    } else {
        pattern.clone()
    };

    Regex::new(&pattern, options.matching.ignore_case)
        .map(Some)
        .map_err(PgrepError::Regex)
}

/// The tasks of `snapshot`: its processes, or with `-w` their threads.
fn tasks(snapshot: &Snapshot, tasks: Tasks) -> Vec<LinuxTask<'_>> {
    snapshot
        .processes
        .iter()
        .flat_map(|process| {
            if tasks == Tasks::Threads {
                process.linux_tasks()
            } else {
                vec![process.linux_task()]
            }
        })
        .collect()
}

/// A task that the selection chose.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Selected<'a> {
    pub(crate) task: LinuxTask<'a>,
    /// The task whose name or command line is in procps-ng's `cmdoutput` buffer
    /// when procps-ng selects `task`, or `None` while the buffer is still
    /// empty. `pkill -e` prints that text. procps-ng allocates the buffer with
    /// `xmalloc`, which does not clear it. glibc normally serves the 128 KiB
    /// request with `mmap`, whose pages are zeroed, so the buffer starts empty
    /// in the reference scenarios, but glibc's allocation tunables can change
    /// that. This implementation always starts with an empty buffer.
    pub(crate) output: Option<LinuxTask<'a>>,
}

/// The task that `-n` or `-o` keeps so far, as procps-ng's
/// `saved_start_time` and `saved_pid` record it.
#[derive(Clone, Copy, Debug)]
struct Saved {
    /// The start tick, or `None` before the first match. procps-ng starts
    /// with 0 for `-n` and the largest tick for `-o`. No process started
    /// before tick 0 or after the largest tick, so the first comparison never
    /// rejects a process.
    start: Option<u64>,
    id: u64,
}

impl Selection<'_> {
    /// The tasks in `snapshot` that pass the selection, as procps-ng's
    /// `select_procs` chooses them, in the snapshot's order of processes and
    /// then in ascending order of thread ID. The tool's own process is never
    /// selected, and with `-A` neither are its ancestors. With `-w`, every
    /// thread of the tool's own process is left out, but only the main
    /// thread of each ancestor, as on Linux.
    pub(crate) fn select<'s>(
        &self,
        snapshot: &'s Snapshot,
        invocation: &Invocation<'_>,
    ) -> Vec<Selected<'s>> {
        select(self.options, self.regex.as_ref(), snapshot, invocation)
    }
}

fn select<'a>(
    options: &Options,
    regex: Option<&Regex>,
    snapshot: &'a Snapshot,
    invocation: &Invocation<'_>,
) -> Vec<Selected<'a>> {
    let clock = Clock::new(snapshot);
    let ancestors = if options.filters.ignore_ancestors {
        ancestors(snapshot, invocation.myself)
    } else {
        BTreeSet::new()
    };
    let mut saved = Saved {
        start: None,
        id: match options.pick {
            Pick::Oldest => u64::MAX,
            Pick::Newest | Pick::All => 0,
        },
    };
    let mut selected = Vec::new();
    let mut output = None;

    for task in tasks(snapshot, options.tasks) {
        let pid = task.process().info.pid;

        // Linux does not expose a process with pid 0 in `/proc`, so procps-ng
        // never lists one. On Darwin, `kernel_task` has pid 0, and `kill 0`
        // signals the caller's process group, so a script that passed a 0 from
        // pgrep to `kill` would signal its own group.
        //
        // procps-ng compares each task's ID with the pids of the tool and its
        // ancestors. On Linux, only a main thread's ID equals a pid, so only
        // the ancestors' main threads are left out. procps-ng's `pgrep` has one
        // thread, so leaving out every thread of the tool's own process matches
        // Linux.
        if pid.as_raw() == 0
            || pid == invocation.myself
            || (task.is_leader() && ancestors.contains(&pid))
        {
            continue;
        }

        let start = clock.tick(task.process().info.start_time);
        let mut matches = match (options.pick, saved.start) {
            (Pick::Newest, Some(saved)) if start < saved => false,
            (Pick::Oldest, Some(saved)) if start > saved => false,
            _ => passes_filters(options, snapshot, clock, &task, invocation.charset),
        };

        // procps-ng refreshes its output buffer under this condition, before it
        // matches the pattern, whether or not it then selects the task.
        if options.output.listing != Listing::Ids
            || options.output.shell_quote
            || (matches && regex.is_some())
        {
            output = Some(task);
        }

        if matches && let Some(regex) = regex {
            let text = if options.matching.full {
                task.command_line(invocation.charset)
            } else {
                task.command(invocation.charset)
            };

            matches = regex.is_match(&text);
        }

        if matches == options.inverse {
            continue;
        }

        if options.pick != Pick::All {
            let id = task.id();
            let tied = saved.start == Some(start);
            let keep_saved = match options.pick {
                Pick::Newest => tied && saved.id > id,
                Pick::Oldest => tied && saved.id < id,
                Pick::All => false,
            };

            if keep_saved {
                continue;
            }

            saved = Saved {
                start: Some(start),
                id,
            };
            selected.clear();
        }

        selected.push(Selected { task, output });
    }

    selected
}

/// Whether `task` passes every filter of `options` other than the pattern.
/// procps-ng checks the filters in this order.
fn passes_filters(
    options: &Options,
    snapshot: &Snapshot,
    clock: Clock,
    task: &LinuxTask<'_>,
    charset: Charset,
) -> bool {
    let process = task.process();
    let info = &process.info;
    let credentials = &info.credentials;
    let listed = |list: &Option<Vec<i64>>, value: i64| {
        list.as_ref().is_none_or(|list| list.contains(&value))
    };

    listed(&options.filters.parents, i64::from(info.ppid.as_raw()))
        && listed(&options.filters.pids, i64::from(info.pid.as_raw()))
        && listed(
            &options.filters.process_groups,
            i64::from(info.pgid.as_raw()),
        )
        && listed(
            &options.filters.effective_users,
            i64::from(credentials.euid.as_raw()),
        )
        && listed(
            &options.filters.real_users,
            i64::from(credentials.ruid.as_raw()),
        )
        && listed(
            &options.filters.real_groups,
            i64::from(credentials.rgid.as_raw()),
        )
        && options.filters.sessions.as_ref().is_none_or(|sessions| {
            session(snapshot, process)
                .is_some_and(|session| sessions.contains(&i64::from(session.as_raw())))
        })
        && options
            .filters
            .older
            .is_none_or(|older| clock.elapsed_seconds(info.start_time) >= i64::from(older))
        && options.filters.terminals.as_ref().is_none_or(|terminals| {
            let name = terminal_name(process);

            terminals
                .iter()
                .any(|terminal| terminal.as_bytes() == name.as_bytes())
        })
        && options.filters.states.as_ref().is_none_or(|states| {
            task.state_letter()
                .and_then(|letter| u8::try_from(letter).ok())
                .is_some_and(|letter| states.as_bytes().contains(&letter))
        })
        && options.filters.environment.as_ref().is_none_or(|entries| {
            process
                .linux_environment(charset)
                .is_some_and(|environment| environment_matches(&environment, entries))
        })
        && (!options.filters.require_handler || info.caught_signals.contains(options.signal))
}

/// Whether `environment` has a variable that one of `entries` describes, as
/// procps-ng's `match_env_list` decides. An entry `NAME=value` matches that
/// exact variable, and an entry `NAME` matches the variable with any value.
/// procps-ng stops reading the environment at its first empty string.
fn environment_matches(environment: &[OsString], entries: &[OsString]) -> bool {
    entries.iter().any(|entry| {
        let entry = entry.as_bytes();

        environment
            .iter()
            .map(|variable| variable.as_bytes())
            .take_while(|variable| !variable.is_empty())
            .any(|variable| {
                if entry.contains(&b'=') {
                    return variable == entry;
                }

                variable.starts_with(entry) && variable.get(entry.len()) == Some(&b'=')
            })
    })
}

/// The ancestors of `myself`, as procps-ng's `get_our_ancestors` finds them:
/// its parent, the parent's parent, and so on. procps-ng's search ends when it
/// finds no process with the pid that it looks for. On Linux, that happens
/// after `init`, whose parent's pid is 0, because Linux does not expose a
/// process with pid 0 in `/proc`. On Darwin, `kernel_task` has pid 0 and is its
/// own parent, so the search would never end. This function therefore also
/// stops at a pid that it has already visited.
fn ancestors(snapshot: &Snapshot, myself: Pid) -> BTreeSet<Pid> {
    let mut ancestors = BTreeSet::new();
    let mut search = myself;

    while let Some(process) = snapshot
        .processes
        .iter()
        .find(|process| process.info.pid == search)
    {
        search = process.info.ppid;

        if !ancestors.insert(search) {
            break;
        }
    }

    ancestors
}

/// Whether `pattern` qualifies for procps-ng's warning that a pattern cannot
/// match a command name: it is longer than a Linux command name and has no `|`
/// or `[`. procps-ng's check is a heuristic: a pattern such as
/// `sshd.?.?.?.?.?.?` qualifies although it matches `sshd`. procps-ng warns
/// only when no process matches and `-f` is not given.
pub(crate) fn is_long_match(pattern: &OsStr) -> bool {
    let bytes = pattern.as_bytes();

    bytes.len() > COMMAND_NAME_LENGTH && !bytes.iter().any(|&byte| byte == b'|' || byte == b'[')
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        time::{Duration, SystemTime},
    };

    use assert_matches::assert_matches;
    use darwin_proc::{
        AccountingFlags, Credentials, Gid, Pid, ProcessFlags, ProcessInfo, RegexErrorKind,
        RunState, SignalSet, Status, Terminal, Uid,
    };
    use pretty_assertions::assert_eq;
    use procps_core::{
        Charset, Field, FieldGroup, Process, Snapshot, SnapshotRequest, testing::thread,
    };
    use rstest::rstest;

    use super::{Invocation, Selection, environment_matches, is_long_match, request};
    use crate::pgrep::{Command, LinuxOption, Mode, Options, PgrepError};

    /// A process for the snapshot.
    struct Fixture {
        pid: i32,
        ppid: i32,
        pgid: i32,
        session: Option<i32>,
        comm: &'static str,
        arguments: &'static [&'static str],
        start: Duration,
        uid: u32,
        status: Status,
    }

    /// The process ID of the tool itself in the snapshot.
    const MYSELF: i32 = 100;

    const fn fixture(pid: i32, parent: i32, comm: &'static str, start: u64, uid: u32) -> Fixture {
        Fixture {
            pid,
            ppid: parent,
            pgid: pid,
            session: Some(if parent == 60 { 60 } else { 1 }),
            comm,
            arguments: &[],
            start: Duration::from_secs(start),
            uid,
            status: Status::Running,
        }
    }

    /// The processes. `kernel_task` is its own parent, as on Darwin. 202 and
    /// 203 are zombies, and the source could not read their sessions. 202 is
    /// in 200's process group, and 203 is alone in its own.
    const FIXTURES: [Fixture; 11] = [
        Fixture {
            session: None,
            ..fixture(0, 0, "kernel_task", 0, 0)
        },
        Fixture {
            arguments: &["/sbin/launchd"],
            ..fixture(1, 0, "launchd", 10, 0)
        },
        Fixture {
            arguments: &["-zsh"],
            ..fixture(60, 1, "zsh", 30, 501)
        },
        Fixture {
            arguments: &["pgrep", "x"],
            ..fixture(MYSELF, 60, "pgrep", 40, 501)
        },
        Fixture {
            arguments: &["sleep", "100"],
            ..fixture(200, 60, "sleep", 50, 501)
        },
        Fixture {
            arguments: &["sleep", "200"],
            status: Status::Stopped,
            ..fixture(201, 60, "sleep", 50, 501)
        },
        Fixture {
            pgid: 200,
            session: None,
            status: Status::Zombie,
            ..fixture(202, 60, "defunct-child", 70, 501)
        },
        Fixture {
            session: None,
            status: Status::Zombie,
            ..fixture(203, 60, "lost-zombie", 75, 503)
        },
        Fixture {
            arguments: &["/usr/bin/Sleepy", "--deep"],
            ..fixture(300, 1, "Sleepy", 20, 502)
        },
        Fixture {
            arguments: &["very-long-name-daemon"],
            ..fixture(400, 1, "very-long-name-", 60, 0)
        },
        fixture(500, 1, "caf\u{e9}", 5, 503),
    ];

    /// The device number of `/dev/null`, for which `devname_r` returns `null`,
    /// so a fixture's terminal has a known name.
    fn null_device() -> Result<libc::dev_t, rustix::io::Errno> {
        Ok(rustix::fs::stat("/dev/null")?.st_rdev)
    }

    fn process(fixture: &Fixture, null: libc::dev_t) -> Process {
        let uid = Uid::from(fixture.uid);
        let gid = Gid::from(fixture.uid);
        let info = ProcessInfo {
            pid: Pid::from(fixture.pid),
            ppid: Pid::from(fixture.ppid),
            pgid: Pid::from(fixture.pgid),
            terminal: (fixture.pid == 201).then_some(Terminal {
                device: null,
                foreground_group: Pid::from(201),
            }),
            credentials: Credentials {
                ruid: uid,
                euid: uid,
                svuid: uid,
                rgid: gid,
                egid: gid,
                svgid: gid,
                groups: vec![gid],
            },
            nice: 0,
            priority: 31,
            status: fixture.status,
            flags: ProcessFlags::default(),
            accounting: AccountingFlags::default(),
            session_leader: false,
            start_time: SystemTime::UNIX_EPOCH + fixture.start,
            comm: fixture.comm.into(),
            ignored_signals: SignalSet::default(),
            caught_signals: SignalSet::from(if fixture.pid == 200 {
                1 << (libc::SIGHUP - 1)
            } else {
                0
            }),
        };
        let session = fixture.session.map_or(Field::Unsupported, |session| {
            Field::Available(Pid::from(session))
        });
        let mut process = Process::from_identity(info, Field::Unsupported, session);
        process.arguments = Some(Field::Available(
            fixture.arguments.iter().map(OsString::from).collect(),
        ));
        process.environment = Some(if fixture.pid == 200 {
            Field::Available(vec![
                OsString::from("HOME=/Users/a"),
                OsString::from("LANG=C"),
            ])
        } else {
            Field::Denied
        });

        process
    }

    fn snapshot() -> Result<Snapshot, Box<dyn std::error::Error>> {
        let null = null_device()?;

        Ok(Snapshot {
            taken: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
            uptime: Duration::from_secs(100),
            processes: FIXTURES
                .iter()
                .map(|fixture| process(fixture, null))
                .collect(),
            system: None,
        })
    }

    fn invocation(charset: Charset) -> Invocation<'static> {
        Invocation {
            mode: Mode::Pgrep,
            program: OsStr::new("pgrep"),
            myself: Pid::from(MYSELF),
            charset,
        }
    }

    fn options(arguments: &[&str]) -> Result<Options, Box<dyn std::error::Error>> {
        match Options::parse(Mode::Pgrep, arguments.iter().map(OsString::from))? {
            Command::Run(options) => Ok(*options),
            other => Err(format!("expected options, got {other:?}").into()),
        }
    }

    fn selected_in(
        arguments: &[&str],
        charset: Charset,
    ) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
        selected_from(&snapshot()?, arguments, charset)
    }

    fn selected_from(
        snapshot: &Snapshot,
        arguments: &[&str],
        charset: Charset,
    ) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
        let options = options(arguments)?;
        let selected = Selection::new(&options)?.select(snapshot, &invocation(charset));

        Ok(selected
            .iter()
            .map(|selected| selected.task.process().info.pid.as_raw())
            .collect())
    }

    fn selected(arguments: &[&str]) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
        selected_in(arguments, Charset::Utf8)
    }

    #[rstest]
    #[case::name(&["pgrep", "sleep"], &[200, 201])]
    #[case::not_myself(&["pgrep", "pgrep"], &[])]
    #[case::regex(&["pgrep", "^(zsh|launchd)$"], &[1, 60])]
    #[case::exact(&["pgrep", "-x", "slee"], &[])]
    #[case::ignore_case(&["pgrep", "-i", "^sleep"], &[200, 201, 300])]
    #[case::full(&["pgrep", "-f", "sleep 2"], &[201])]
    #[case::full_exact(&["pgrep", "-f", "-x", "sleep"], &[])]
    #[case::full_without_arguments(&["pgrep", "-f", "^\\[caf"], &[500])]
    #[case::kernel_task_by_name(&["pgrep", "-x", "kernel_task"], &[])]
    #[case::kernel_task_by_pid(&["pgrep", "-p", "0"], &[])]
    #[case::kernel_task_as_parent(&["pgrep", "-P", "0"], &[1])]
    #[case::inverse(&["pgrep", "-v", "e"], &[1, 60, 500])]
    #[case::newest(&["pgrep", "-n", "sleep"], &[201])]
    #[case::oldest(&["pgrep", "-o", "sleep"], &[200])]
    #[case::newest_of_all(&["pgrep", "-n"], &[203])]
    #[case::parent(&["pgrep", "-P", "60"], &[200, 201, 202, 203])]
    #[case::pid(&["pgrep", "-p", "1,300,999"], &[1, 300])]
    #[case::process_group(&["pgrep", "-g", "200"], &[200, 202])]
    #[case::effective_user(&["pgrep", "-u", "502", "S"], &[300])]
    #[case::real_user(&["pgrep", "-U", "0"], &[1, 400])]
    #[case::real_group(&["pgrep", "-G", "501", "sleep"], &[200, 201])]
    #[case::session(&["pgrep", "-s", "60"], &[200, 201, 202])]
    #[case::older(&["pgrep", "-O", "50"], &[1, 60, 200, 201, 300, 500])]
    #[case::terminal(&["pgrep", "-t", "null"], &[201])]
    #[case::no_terminal(&["pgrep", "-t", "?", "sleep"], &[200])]
    #[case::stopped(&["pgrep", "-r", "T"], &[201])]
    #[case::zombie(&["pgrep", "-r", "Z"], &[202, 203])]
    #[case::unknown_state(&["pgrep", "-r", "-"], &[])]
    #[case::environment_name(&["pgrep", "--env", "LANG"], &[200])]
    #[case::environment_value(&["pgrep", "--env", "HOME=/Users/b,LANG=C"], &[200])]
    #[case::environment_prefix(&["pgrep", "--env", "LAN"], &[])]
    #[case::environment_prefix_of_value(&["pgrep", "--env", "HOME=/Users"], &[])]
    #[case::handler(&["pgrep", "-H", "--signal", "HUP", "sleep"], &[200])]
    fn processes_are_selected_as_procps_ng_selects_them(
        #[case] arguments: &[&str],
        #[case] expected: &[i32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(selected(arguments)?, expected);

        Ok(())
    }

    // The snapshot is taken 100 seconds after boot. 210 and 211 start in the
    // same tick, and 212 starts 49.9999 seconds before the snapshot, in a tick
    // that starts 50 seconds before it. 213 starts after 212 and comes after it
    // in the snapshot. 215 and 214 start in the same tick, and 215 comes first.
    #[rstest]
    #[case::newest(&["pgrep", "-n", "ticker"], &[211])]
    #[case::oldest(&["pgrep", "-o", "ticker"], &[210])]
    #[case::older(&["pgrep", "-O", "50", "aged"], &[212])]
    #[case::oldest_before_a_younger_process(&["pgrep", "-o", "aged"], &[212])]
    #[case::newest_with_the_higher_pid_first(&["pgrep", "-n", "twin"], &[215])]
    #[case::oldest_with_the_higher_pid_first(&["pgrep", "-o", "twin"], &[214])]
    fn start_times_are_compared_in_clock_ticks(
        #[case] arguments: &[&str],
        #[case] expected: &[i32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let null = null_device()?;
        let mut snapshot = snapshot()?;
        snapshot.processes.extend(
            [
                Fixture {
                    start: Duration::from_millis(80_009),
                    ..fixture(210, 1, "ticker", 0, 501)
                },
                Fixture {
                    start: Duration::from_millis(80_001),
                    ..fixture(211, 1, "ticker", 0, 501)
                },
                Fixture {
                    start: Duration::from_micros(50_000_100),
                    ..fixture(212, 1, "aged", 0, 501)
                },
                fixture(213, 1, "aged", 90, 501),
                fixture(215, 1, "twin", 85, 501),
                fixture(214, 1, "twin", 85, 501),
            ]
            .iter()
            .map(|fixture| process(fixture, null)),
        );

        assert_eq!(
            selected_from(&snapshot, arguments, Charset::Utf8)?,
            expected
        );

        Ok(())
    }

    /// The IDs of the tasks that `arguments` select, when the source has
    /// read the threads of the tool, its ancestor 60 and the processes 200,
    /// 201 and 300. 200 and 201 started at the same time. The source could
    /// not read the arguments of 300, whose second thread has its own name.
    fn selected_tasks(arguments: &[&str]) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
        let mut snapshot = snapshot()?;

        for process in &mut snapshot.processes {
            let threads = match process.info.pid.as_raw() {
                60 => vec![
                    thread(600, b"", RunState::Waiting),
                    thread(601, b"", RunState::Waiting),
                ],
                MYSELF => vec![
                    thread(1001, b"", RunState::Running),
                    thread(1000, b"", RunState::Running),
                ],
                200 => vec![
                    thread(2001, b"io", RunState::Running),
                    thread(2000, b"main", RunState::Waiting),
                    thread(2002, b"", RunState::Uninterruptible),
                ],
                201 => vec![thread(2010, b"", RunState::Running)],
                300 => {
                    process.arguments = Some(Field::Denied);
                    vec![
                        thread(3000, b"", RunState::Waiting),
                        thread(3001, b"worker", RunState::Waiting),
                    ]
                }
                _ => continue,
            };
            process.threads = Some(Field::Available(threads));
        }

        let options = options(arguments)?;
        let selected = Selection::new(&options)?.select(&snapshot, &invocation(Charset::Utf8));

        Ok(selected.iter().map(|selected| selected.task.id()).collect())
    }

    #[rstest]
    #[case::names(&["pgrep", "-w", "sleep"], &[2000, 2002, 2010])]
    #[case::thread_name(&["pgrep", "-w", "^io$"], &[2001])]
    #[case::running(&["pgrep", "-w", "-r", "R"], &[2001])]
    #[case::uninterruptible(&["pgrep", "-w", "-r", "D"], &[2002])]
    #[case::stopped(&["pgrep", "-w", "-r", "T"], &[2010])]
    #[case::not_myself(&["pgrep", "-w", "-U", "501"], &[600, 601, 2000, 2001, 2002, 2010, 202])]
    #[case::not_ancestors_main_threads(&["pgrep", "-w", "-A", "-U", "501"], &[601, 2000, 2001, 2002, 2010, 202])]
    #[case::pid(&["pgrep", "-w", "-p", "200"], &[2000, 2001, 2002])]
    #[case::newest(&["pgrep", "-w", "-n", "sleep"], &[2010])]
    #[case::oldest(&["pgrep", "-w", "-o", "sleep"], &[2000])]
    #[case::unreadable_command_line_of_a_named_thread(&["pgrep", "-w", "-f", "^\\[worker\\]$"], &[3001])]
    #[case::unreadable_command_line_of_the_main_thread(&["pgrep", "-w", "-f", "^\\[Sleepy\\]$"], &[3000])]
    fn threads_are_selected_as_tasks(
        #[case] arguments: &[&str],
        #[case] expected: &[u64],
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(selected_tasks(arguments)?, expected);

        Ok(())
    }

    #[test]
    fn ignore_ancestors_leaves_out_the_tool_s_ancestors() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(
            (
                selected(&["pgrep", "-U", "0,501"])?,
                selected(&["pgrep", "-A", "-U", "0,501"])?
            ),
            (vec![1, 60, 200, 201, 202, 400], vec![200, 201, 202, 400])
        );

        Ok(())
    }

    // The test process keeps the C locale, so `regexec` matches bytes, and the
    // patterns test only the escaping.
    #[rstest]
    #[case::utf8_kept(Charset::Utf8, "^caf[?][?]$", &[])]
    #[case::other_replaced(Charset::from_codeset("US-ASCII"), "^caf[?][?]$", &[500])]
    fn names_are_escaped_for_the_charset(
        #[case] charset: Charset,
        #[case] pattern: &str,
        #[case] expected: &[i32],
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(selected_in(&["pgrep", pattern], charset)?, expected);

        Ok(())
    }

    #[test]
    fn an_invalid_pattern_is_a_regex_error() -> Result<(), Box<dyn std::error::Error>> {
        let options = options(&["pgrep", "("])?;

        assert_matches!(
            Selection::new(&options),
            Err(PgrepError::Regex(error)) if error.kind == RegexErrorKind::Parenthesis
        );

        Ok(())
    }

    #[rstest]
    #[case::invalid_pattern(&["pgrep", "--cgroup", "a", "("], Some(RegexErrorKind::Parenthesis))]
    #[case::valid_pattern(&["pgrep", "--cgroup", "a", "x"], None)]
    fn a_linux_option_is_refused_after_the_pattern_is_compiled(
        #[case] arguments: &[&str],
        #[case] regex_error: Option<RegexErrorKind>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let options = options(arguments)?;

        match regex_error {
            Some(kind) => assert_matches!(
                Selection::new(&options),
                Err(PgrepError::Regex(error)) if error.kind == kind
            ),
            None => assert_matches!(
                Selection::new(&options),
                Err(PgrepError::Unsupported(LinuxOption::Cgroup))
            ),
        }

        Ok(())
    }

    #[rstest]
    #[case::identities(&["pgrep", "x"], SnapshotRequest::default())]
    #[case::full(&["pgrep", "-f", "x"], SnapshotRequest::default().with(FieldGroup::Arguments))]
    #[case::environment(&["pgrep", "--env", "A"], SnapshotRequest::default().with(FieldGroup::Environment))]
    #[case::states(&["pgrep", "-r", "S"], SnapshotRequest::default().with(FieldGroup::Threads))]
    #[case::command_lines(&["pgrep", "-a", "x"], SnapshotRequest::default().with(FieldGroup::Arguments))]
    #[case::threads(&["pgrep", "-w", "x"], SnapshotRequest::default().with(FieldGroup::Threads))]
    fn the_request_asks_only_for_what_the_options_need(
        #[case] arguments: &[&str],
        #[case] expected: SnapshotRequest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(request(&options(arguments)?), expected);

        Ok(())
    }

    #[rstest]
    #[case::name(&["A=1", "B=2"], "B", true)]
    #[case::after_empty_string(&["A=1", "", "B=2"], "B", false)]
    #[case::before_empty_string(&["A=1", "", "B=2"], "A", true)]
    fn the_environment_is_read_up_to_its_first_empty_string(
        #[case] environment: &[&str],
        #[case] entry: &str,
        #[case] expected: bool,
    ) {
        let environment: Vec<OsString> = environment.iter().map(OsString::from).collect();

        assert_eq!(
            environment_matches(&environment, &[OsString::from(entry)]),
            expected
        );
    }

    #[rstest]
    #[case::long("very-long-name-daemon", true)]
    #[case::fifteen("very-long-name-", false)]
    #[case::alternation("very-long-name-daemon|x", false)]
    #[case::bracket("very-long-name-[d]aemon", false)]
    fn a_long_pattern_is_detected_as_procps_ng_detects_it(
        #[case] pattern: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(is_long_match(pattern.as_ref()), expected);
    }
}
