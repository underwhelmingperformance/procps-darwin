// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2025 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/parser.c and src/ps/select.c.

use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use procps_core::{Charset, FieldGroup, LinuxTask, Process, Snapshot};

use crate::{
    c_number::{Base, whole_strtoul},
    linux,
    ps::{error::SyntaxError, personality, system::System},
};

/// The longest command name that `-C` keeps. procps-ng copies each name into
/// a buffer of 64 bytes, and the last is the NUL.
const COMMAND_MAX: usize = 63;

/// The longest path that procps-ng builds to look a terminal up. Its buffer
/// has 4096 bytes, and the last is the NUL.
const PATH_MAX: usize = 4095;

/// The largest pid that procps-ng accepts.
const PID_LIMIT: u64 = 0x7fff_ffff;

/// The largest user or group ID that procps-ng accepts, one less than `-1`
/// as an ID.
const ID_LIMIT: u64 = 0xffff_fffe;

/// A terminal that a selection list specifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Terminal {
    /// The terminal with this device number.
    Device(libc::dev_t),
    /// No terminal: `-` or `?`.
    None,
    /// The controlling terminal of `ps` itself, which BSD's `T` and `t`
    /// without an argument select.
    Own,
}

/// The values of one selection option, which select a process if any value
/// matches it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Selector {
    /// Real user IDs (`-U`, `--User`).
    RealUsers(Vec<u32>),
    /// Effective user IDs (`-u`, `U`, `--user`).
    EffectiveUsers(Vec<u32>),
    /// Real group IDs (`-G`, `--Group`).
    RealGroups(Vec<u32>),
    /// Effective group IDs (`-g`, `--group`).
    EffectiveGroups(Vec<u32>),
    /// Process group IDs (a trailing `-<pgid>`).
    ProcessGroups(Vec<i32>),
    /// Process IDs (`-p`, `p`, `--pid` and trailing pids).
    Pids(Vec<i32>),
    /// Process IDs that `ps` reads directly, without listing every process
    /// (`-q`, `q`, `--quick-pid`, and `-p` on its own).
    QuickPids(Vec<i32>),
    /// Parent process IDs (`--ppid`).
    Parents(Vec<i32>),
    /// Terminals (`-t`, `t`, `T`, `--tty`).
    Terminals(Vec<Terminal>),
    /// Session IDs (`-s`, `--sid`, `-g` and a trailing `+<sid>`).
    Sessions(Vec<i32>),
    /// Command names, cut to 63 bytes (`-C`).
    Commands(Vec<Vec<u8>>),
}

/// A selection option as the parser reads it, before procps-ng decides
/// whether `-p` reads its processes directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Listed {
    /// A selector.
    Selector(Selector),
    /// The process IDs of `-p`, `p` or `--pid`, which `ps` reads directly if
    /// no other option needs every process (`SEL_PID_TRY_QUICK`).
    TryQuickPids(Vec<i32>),
}

bitflags::bitflags! {
    /// The selection options without values (`SS_*`).
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct SimpleSelect: u32 {
        /// BSD's `x`.
        const B_X = 0x01;
        /// BSD's `g`.
        const B_G = 0x02;
        /// UNIX's `-d`.
        const U_D = 0x04;
        /// UNIX's `-a`.
        const U_A = 0x08;
        /// BSD's `a`.
        const B_A = 0x10;
    }
}

bitflags::bitflags! {
    /// The tests that the selection options without values apply to a
    /// process, as procps-ng's `select_setup` decides them.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct SimpleTests: u32 {
        /// A selection option without values was given, so the tests select
        /// processes even when selection lists are given.
        const GIVEN = 0x01;
        /// Select only the processes with the effective user ID of `ps`.
        const OWN_USER = 0x02;
        /// Select only the processes on the controlling terminal of `ps`.
        const OWN_TERMINAL = 0x04;
        /// Leave out session leaders.
        const NO_LEADERS = 0x08;
        /// Select only the processes that have a controlling terminal.
        const HAS_TERMINAL = 0x10;
    }
}

/// The processes that the options select.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Selection {
    /// `-A`, `-e`, or BSD's `a` with `x`: select every process.
    pub(crate) all: bool,
    /// The tests of the selection options without values.
    pub(crate) simple: SimpleTests,
    /// The selection lists, in the order of the command line.
    pub(crate) lists: Vec<Selector>,
    /// BSD's `r`: select only running processes.
    pub(crate) running_only: bool,
    /// `-N` and `--deselect`: select the processes that the other options
    /// do not select.
    pub(crate) negate: bool,
}

/// What the selection needs to know about `ps` itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Myself {
    /// The effective user ID of `ps`.
    pub(crate) euid: u32,
    /// The device number of the controlling terminal of `ps`, or `None`
    /// without one.
    pub(crate) terminal: Option<libc::dev_t>,
}

impl Selection {
    /// The pids that `ps` reads directly, in the order of the command line,
    /// if the selection is `-q` or a `-p` that procps-ng reads directly.
    pub(crate) fn quick_pids(&self) -> Option<&[i32]> {
        self.lists.iter().find_map(|selector| match selector {
            Selector::QuickPids(pids) => Some(pids.as_slice()),
            _ => None,
        })
    }

    /// The field groups that the selection needs. BSD's `r` needs the
    /// threads, because they decide whether a process is running.
    pub(crate) fn groups(&self) -> &'static [FieldGroup] {
        if self.running_only {
            &[FieldGroup::Threads]
        } else {
            &[]
        }
    }

    /// Whether the selection accepts `task` before BSD's `r` and `-N`
    /// apply, as the first part of procps-ng's `want_this_proc` decides.
    /// `-A`, `-e` and BSD's `ax` accept every task. Otherwise, the tests of
    /// the selection options without values apply when one of those options
    /// was given, and also when no selection list was given. A task is
    /// accepted if it passes those tests or if one of the lists matches it.
    pub(crate) fn accepts(
        &self,
        snapshot: &Snapshot,
        task: &LinuxTask<'_>,
        myself: Myself,
        charset: Charset,
    ) -> bool {
        self.all
            || ((self.simple.contains(SimpleTests::GIVEN) || self.lists.is_empty())
                && self.simple_accept(task.process(), myself))
            || self
                .lists
                .iter()
                .any(|selector| selector.matches(snapshot, task, myself, charset))
    }

    /// Whether the selection can accept a thread and not its process, or the
    /// reverse. Only `-C` compares a value that a thread has for itself: its
    /// name.
    pub(crate) fn depends_on_threads(&self) -> bool {
        self.lists
            .iter()
            .any(|selector| matches!(selector, Selector::Commands(_)))
    }

    /// Whether a process might be selected, given whether the selection
    /// accepts it. With BSD's `r`, the selection also depends on the
    /// process's state, which `selects` decides after the requested data
    /// has been read.
    pub(crate) fn may_select(&self, accepted: bool) -> bool {
        if self.running_only {
            accepted || self.negate
        } else {
            accepted != self.negate
        }
    }

    /// Whether the selection selects `task`, given whether it accepts it, as
    /// the end of procps-ng's `want_this_proc` decides. BSD's `r` keeps the
    /// tasks that are running or in an uninterruptible wait, and `-N` then
    /// inverts the result.
    pub(crate) fn selects(&self, accepted: bool, task: &LinuxTask<'_>) -> bool {
        let running = matches!(task.state_letter(), Some('R' | 'D'));

        (accepted && (running || !self.running_only)) != self.negate
    }

    /// Whether `process` passes the tests of the selection options without
    /// values, as procps-ng's `simple_accept` decides.
    fn simple_accept(&self, process: &Process, myself: Myself) -> bool {
        let info = &process.info;
        let terminal = info.terminal.map(|terminal| terminal.device);
        let tests = self.simple;

        !(tests.contains(SimpleTests::OWN_USER) && info.credentials.euid.as_raw() != myself.euid
            || tests.contains(SimpleTests::OWN_TERMINAL) && terminal != myself.terminal
            || tests.contains(SimpleTests::NO_LEADERS) && info.session_leader
            || tests.contains(SimpleTests::HAS_TERMINAL) && terminal.is_none())
    }
}

impl Selector {
    /// Whether one of the selector's values matches `task`, as procps-ng's
    /// `proc_was_listed` decides. A thread matches `-C` by its own name.
    fn matches(
        &self,
        snapshot: &Snapshot,
        task: &LinuxTask<'_>,
        myself: Myself,
        charset: Charset,
    ) -> bool {
        let process = task.process();
        let info = &process.info;
        let credentials = &info.credentials;

        match self {
            Self::RealUsers(users) => users.contains(&credentials.ruid.as_raw()),
            Self::EffectiveUsers(users) => users.contains(&credentials.euid.as_raw()),
            Self::RealGroups(groups) => groups.contains(&credentials.rgid.as_raw()),
            Self::EffectiveGroups(groups) => groups.contains(&credentials.egid.as_raw()),
            Self::ProcessGroups(groups) => groups.contains(&info.pgid.as_raw()),
            Self::Pids(pids) | Self::QuickPids(pids) => pids.contains(&info.pid.as_raw()),
            Self::Parents(parents) => parents.contains(&info.ppid.as_raw()),
            Self::Sessions(sessions) => linux::session(snapshot, process)
                .is_some_and(|session| sessions.contains(&session.as_raw())),
            Self::Terminals(terminals) => {
                let device = info.terminal.map(|terminal| terminal.device);

                terminals.iter().any(|terminal| match terminal {
                    Terminal::Device(selected) => device == Some(*selected),
                    Terminal::None => device.is_none(),
                    Terminal::Own => device == myself.terminal,
                })
            }
            Self::Commands(commands) => {
                let name = task.command(charset);

                commands
                    .iter()
                    .any(|command| command_matches(name.as_bytes(), command))
            }
        }
    }
}

/// Whether the command name `name` matches `command` from `-C`, as
/// procps-ng compares them. A name of 15 bytes, the most that Linux keeps,
/// also matches a longer command that starts with it.
fn command_matches(name: &[u8], command: &[u8]) -> bool {
    if name.len() == 15 && command.len() >= 15 && name == &command[..15] {
        return true;
    }

    name == command
}

/// Decides the tests of the selection options without values, as procps-ng's
/// `select_setup` does. BSD's `a` with `x` selects every process, so it sets
/// `all` and gives no tests.
pub(crate) fn select_setup(
    simple_select: SimpleSelect,
    flags: personality::Flags,
    prefer_bsd_defaults: bool,
    all: &mut bool,
) -> Result<SimpleTests, SyntaxError> {
    let mut tests = SimpleTests::empty();

    tests.set(SimpleTests::GIVEN, !simple_select.is_empty());
    tests.set(
        SimpleTests::NO_LEADERS,
        flags.contains(personality::Flags::NO_DEFAULT_G)
            && !simple_select.contains(SimpleSelect::B_G),
    );

    let (a, d, bsd_a, g, x) = (
        SimpleSelect::U_A,
        SimpleSelect::U_D,
        SimpleSelect::B_A,
        SimpleSelect::B_G,
        SimpleSelect::B_X,
    );
    let select = simple_select;

    if select.is_empty() {
        tests |= SimpleTests::OWN_USER;
        tests |= if prefer_bsd_defaults {
            SimpleTests::HAS_TERMINAL
        } else {
            SimpleTests::OWN_TERMINAL
        };
    } else if select == a {
        tests |= SimpleTests::NO_LEADERS | SimpleTests::HAS_TERMINAL;
    } else if select == d || select == a | d {
        tests |= SimpleTests::NO_LEADERS;
    } else if select == bsd_a || select == bsd_a | g {
        tests |= SimpleTests::HAS_TERMINAL;
    } else if select == x || select == x | g {
        tests |= SimpleTests::OWN_USER;
    } else if select == bsd_a | x || select == bsd_a | x | g {
        tests.remove(SimpleTests::GIVEN);
        *all = true;
    } else {
        return Err(SyntaxError::SelectionConflict);
    }

    Ok(tests)
}

/// Splits `list` into its elements and reads each with `parse`, as
/// procps-ng's `parse_list` does. The elements are separated by single
/// spaces, commas or tabs, and none may be empty.
pub(crate) fn parse_list<T>(
    list: &[u8],
    mut parse: impl FnMut(&[u8]) -> Result<T, SyntaxError>,
) -> Result<Vec<T>, SyntaxError> {
    let separator = |byte: &u8| matches!(byte, b' ' | b',' | b'\t');

    if list.is_empty()
        || list.first().is_some_and(separator)
        || list.last().is_some_and(separator)
        || list
            .windows(2)
            .any(|pair| separator(&pair[0]) && separator(&pair[1]))
    {
        return Err(SyntaxError::ImproperList);
    }

    list.split(separator).map(&mut parse).collect()
}

/// Reads a pid as procps-ng's `parse_pid` does: a number in C's base 0
/// from 1 to `0x7fffffff`.
pub(crate) fn parse_pid(text: &[u8]) -> Result<i32, SyntaxError> {
    let number = whole_strtoul(text, Base::Prefixed).ok_or(SyntaxError::PidSyntax)?;

    if !(1..=PID_LIMIT).contains(&number) {
        return Err(SyntaxError::PidRange);
    }

    i32::try_from(number).map_err(|_| SyntaxError::PidRange)
}

/// Reads a parent pid as procps-ng's `parse_ppid` does: a number in C's base
/// 0 from 0 to `0x7fffffff`.
pub(crate) fn parse_ppid(text: &[u8]) -> Result<i32, SyntaxError> {
    let number = whole_strtoul(text, Base::Prefixed).ok_or(SyntaxError::PidSyntax)?;

    if number > PID_LIMIT {
        return Err(SyntaxError::PidRange);
    }

    i32::try_from(number).map_err(|_| SyntaxError::PidRange)
}

/// Reads a user or group as procps-ng's `parse_uid` and `parse_gid` do: a
/// number in C's base 0, or a name that `lookup` finds. With `-N` already
/// given, an unknown name is `-1` and a number of any size is accepted, and
/// the value is cut to 32 bits as procps-ng's `uid_t` cuts it.
fn parse_id(
    text: &[u8],
    negated: bool,
    lookup: impl Fn(&OsStr) -> Option<u32>,
    unknown: SyntaxError,
    out_of_range: SyntaxError,
) -> Result<u32, SyntaxError> {
    let number = match whole_strtoul(text, Base::Prefixed) {
        Some(number) => number,
        None => match lookup(OsStr::from_bytes(text)) {
            Some(id) => u64::from(id),
            None if negated => u64::MAX,
            None => return Err(unknown),
        },
    };

    if !negated && number > ID_LIMIT {
        return Err(out_of_range);
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "procps-ng stores the `unsigned long` in a 32-bit `uid_t` or `gid_t`"
    )]
    Ok(number as u32)
}

/// Reads a user, as procps-ng's `parse_uid` does.
pub(crate) fn parse_uid(
    text: &[u8],
    negated: bool,
    system: &impl System,
) -> Result<u32, SyntaxError> {
    parse_id(
        text,
        negated,
        |name| system.user(name),
        SyntaxError::UnknownUser,
        SyntaxError::UserRange,
    )
}

/// Reads a group, as procps-ng's `parse_gid` does.
pub(crate) fn parse_gid(
    text: &[u8],
    negated: bool,
    system: &impl System,
) -> Result<u32, SyntaxError> {
    parse_id(
        text,
        negated,
        |name| system.group(name),
        SyntaxError::UnknownGroup,
        SyntaxError::GroupRange,
    )
}

/// Reads a command name, as procps-ng's `parse_cmd` does: cut to 63 bytes.
pub(crate) fn parse_command(text: &[u8]) -> Vec<u8> {
    text[..text.len().min(COMMAND_MAX)].to_vec()
}

/// Reads a terminal, as procps-ng's `parse_tty` does. An absolute path must
/// exist. Another name is looked for under `/dev/pts`, `/dev`, as `/dev/tty`
/// and `/dev/pty` followed by the name, and as `/dev/<name>nsole`. Failing
/// those, `-` and `?` mean no terminal. So does any other single character
/// that is the name of a file in the working directory, because procps-ng
/// assumes that the shell expanded `?` to that file's name.
pub(crate) fn parse_tty(text: &[u8], system: &impl System) -> Result<Terminal, SyntaxError> {
    let found = if text.first() == Some(&b'/') {
        Some(
            system
                .file(OsStr::from_bytes(text))
                .ok_or(SyntaxError::TerminalNotFound)?,
        )
    } else {
        [&b"/dev/pts/"[..], b"/dev/", b"/dev/tty", b"/dev/pty"]
            .iter()
            .map(|prefix| [*prefix, text].concat())
            .chain([[b"/dev/", text, b"nsole"].concat()])
            .find_map(|mut path| {
                path.truncate(PATH_MAX);
                system.file(OsStr::from_bytes(&path))
            })
    };

    let Some(found) = found else {
        let no_terminal = text == b"-"
            || text == b"?"
            || (text.len() == 1 && system.file(OsStr::from_bytes(text)).is_some());

        return if no_terminal {
            Ok(Terminal::None)
        } else {
            Err(SyntaxError::TerminalNotFound)
        };
    };

    if !found.character_device {
        return Err(SyntaxError::NotATerminal);
    }

    Ok(Terminal::Device(found.device))
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{
        SimpleSelect, SimpleTests, Terminal, parse_list, parse_pid, parse_ppid, parse_tty,
        parse_uid, select_setup,
    };
    use crate::ps::{
        error::SyntaxError,
        personality::Flags,
        system::{FileStatus, testing::FakeSystem},
    };

    #[rstest]
    #[case::commas("1,2", Ok(vec![1, 2]))]
    #[case::blanks("1 2\t3", Ok(vec![1, 2, 3]))]
    #[case::hexadecimal_and_octal("0x10,010", Ok(vec![16, 8]))]
    #[case::empty("", Err(SyntaxError::ImproperList))]
    #[case::leading(",1", Err(SyntaxError::ImproperList))]
    #[case::trailing("1,", Err(SyntaxError::ImproperList))]
    #[case::double("1,,2", Err(SyntaxError::ImproperList))]
    #[case::comma_and_space("1, 2", Err(SyntaxError::ImproperList))]
    #[case::syntax("1,x", Err(SyntaxError::PidSyntax))]
    #[case::not_octal("08", Err(SyntaxError::PidSyntax))]
    #[case::first_error_wins("0,x", Err(SyntaxError::PidRange))]
    #[case::zero("0", Err(SyntaxError::PidRange))]
    #[case::negative("-1", Err(SyntaxError::PidRange))]
    #[case::too_large("2147483648", Err(SyntaxError::PidRange))]
    #[case::largest("2147483647", Ok(vec![2_147_483_647]))]
    fn a_pid_list_is_read_as_procps_ng_reads_it(
        #[case] list: &str,
        #[case] expected: Result<Vec<i32>, SyntaxError>,
    ) {
        assert_eq!(parse_list(list.as_bytes(), parse_pid), expected);
    }

    #[rstest]
    #[case::zero("0", Ok(0))]
    #[case::empty_after_sign("", Ok(0))]
    #[case::largest("2147483647", Ok(2_147_483_647))]
    #[case::too_large("0x80000000", Err(SyntaxError::PidRange))]
    #[case::negative("-1", Err(SyntaxError::PidRange))]
    fn a_parent_may_be_0(#[case] text: &str, #[case] expected: Result<i32, SyntaxError>) {
        assert_eq!(parse_ppid(text.as_bytes()), expected);
    }

    #[rstest]
    #[case::number("501", false, Ok(501))]
    #[case::name("root", false, Ok(0))]
    #[case::unknown("x", false, Err(SyntaxError::UnknownUser))]
    #[case::unknown_negated("x", true, Ok(u32::MAX))]
    #[case::largest("4294967294", false, Ok(4_294_967_294))]
    #[case::too_large("4294967295", false, Err(SyntaxError::UserRange))]
    #[case::cut_when_negated("4294967296", true, Ok(0))]
    fn a_user_is_read_as_procps_ng_reads_it(
        #[case] text: &str,
        #[case] negated: bool,
        #[case] expected: Result<u32, SyntaxError>,
    ) {
        assert_eq!(
            parse_uid(text.as_bytes(), negated, &FakeSystem::default()),
            expected
        );
    }

    #[rstest]
    #[case::short_name("s001", Ok(Terminal::Device(0x1000_0001)))]
    #[case::path("/dev/ttys001", Ok(Terminal::Device(0x1000_0001)))]
    #[case::console("co", Ok(Terminal::Device(0)))]
    #[case::dash("-", Ok(Terminal::None))]
    #[case::question_mark("?", Ok(Terminal::None))]
    #[case::not_a_terminal("/etc/passwd", Err(SyntaxError::NotATerminal))]
    #[case::missing_path("/dev/nothing", Err(SyntaxError::TerminalNotFound))]
    #[case::unknown("x", Err(SyntaxError::TerminalNotFound))]
    fn a_terminal_is_looked_up_as_procps_ng_looks_it_up(
        #[case] text: &str,
        #[case] expected: Result<Terminal, SyntaxError>,
    ) {
        assert_eq!(parse_tty(text.as_bytes(), &FakeSystem::default()), expected);
    }

    #[test]
    fn a_single_character_that_names_a_file_is_no_terminal() {
        let mut system = FakeSystem::default();

        system.files.insert(
            "x",
            FileStatus {
                character_device: false,
                device: 0,
            },
        );

        assert_eq!(parse_tty(b"x", &system), Ok(Terminal::None));
    }

    #[rstest]
    #[case::none_unix(SimpleSelect::empty(), false, (SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL, false))]
    #[case::none_bsd(SimpleSelect::empty(), true, (SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL, false))]
    #[case::unix_a(SimpleSelect::U_A, false, (SimpleTests::GIVEN | SimpleTests::NO_LEADERS | SimpleTests::HAS_TERMINAL, false))]
    #[case::unix_d(SimpleSelect::U_D, false, (SimpleTests::GIVEN | SimpleTests::NO_LEADERS, false))]
    #[case::unix_a_and_d(SimpleSelect::U_A | SimpleSelect::U_D, false, (SimpleTests::GIVEN | SimpleTests::NO_LEADERS, false))]
    #[case::bsd_a(SimpleSelect::B_A, true, (SimpleTests::GIVEN | SimpleTests::HAS_TERMINAL, false))]
    #[case::bsd_a_g(SimpleSelect::B_A | SimpleSelect::B_G, true, (SimpleTests::GIVEN | SimpleTests::HAS_TERMINAL, false))]
    #[case::bsd_x(SimpleSelect::B_X, true, (SimpleTests::GIVEN | SimpleTests::OWN_USER, false))]
    #[case::bsd_a_x(SimpleSelect::B_A | SimpleSelect::B_X, true, (SimpleTests::empty(), true))]
    #[case::bsd_a_x_g(SimpleSelect::B_A | SimpleSelect::B_X | SimpleSelect::B_G, true, (SimpleTests::empty(), true))]
    fn the_simple_selections_decide_the_tests(
        #[case] select: SimpleSelect,
        #[case] prefer_bsd_defaults: bool,
        #[case] expected: (SimpleTests, bool),
    ) {
        let mut all = false;
        let tests = select_setup(select, Flags::empty(), prefer_bsd_defaults, &mut all);

        assert_eq!(tests.map(|tests| (tests, all)), Ok(expected));
    }

    #[rstest]
    #[case::g_alone(SimpleSelect::B_G)]
    #[case::unix_d_and_bsd_x(SimpleSelect::U_D | SimpleSelect::B_X)]
    #[case::unix_a_and_bsd_a(SimpleSelect::U_A | SimpleSelect::B_A)]
    fn conflicting_simple_selections_are_an_error(#[case] select: SimpleSelect) {
        let mut all = false;

        assert_matches!(
            select_setup(select, Flags::empty(), false, &mut all),
            Err(SyntaxError::SelectionConflict)
        );
    }

    #[test]
    fn sunos_leaves_out_session_leaders_without_g() {
        let mut all = false;
        let without_g = select_setup(SimpleSelect::B_X, Flags::NO_DEFAULT_G, true, &mut all);
        let with_g = select_setup(
            SimpleSelect::B_X | SimpleSelect::B_G,
            Flags::NO_DEFAULT_G,
            true,
            &mut all,
        );

        assert_eq!(
            (without_g, with_g),
            (
                Ok(SimpleTests::GIVEN | SimpleTests::OWN_USER | SimpleTests::NO_LEADERS),
                Ok(SimpleTests::GIVEN | SimpleTests::OWN_USER)
            )
        );
    }
}
