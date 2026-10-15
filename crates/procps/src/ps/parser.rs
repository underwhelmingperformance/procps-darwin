// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2025 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/parser.c, and from the parts of
// src/ps/global.c and src/ps/display.c that run before ps lists processes.

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
};

use crate::{
    c_number::{Base, whole_strtol},
    ps::{
        error::{InternalError, PsError, QuickPidConflict, SyntaxError, Warning},
        format::{FormatError, Formats, Settings, SortFormat, Source},
        help::Section,
        options::{
            Command, Forest, FormatFlags, FormatModifiers, Header, Info, Modifiers, Options,
            Screen, ThreadFlags,
        },
        personality::{self, Personality},
        selection::{
            self, Listed, Selection, Selector, SimpleSelect, SimpleTests, Terminal, parse_list,
        },
        system::System,
    },
};

/// The size of procps-ng's output buffer. It is the initial width of a line
/// when standard output is not a terminal, and the width that `ww` gives.
const OUTBUF_SIZE: i64 = 2 * 64 * 1024;

/// The longest long option that procps-ng looks up. Its buffer has 16 bytes,
/// and the last is the NUL.
const LONG_OPTION_MAX: usize = 15;

/// The largest number of columns or rows that `--cols` and `--rows` accept,
/// plus one.
const SIZE_LIMIT: i64 = 2_000_000_000;

/// How procps-ng reads an argument, as its `arg_type` decides from the first
/// bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    /// A long option, such as `--pid`.
    Gnu,
    /// UNIX options, such as `-ef`.
    Unix,
    /// BSD options, such as `aux`.
    Bsd,
    /// The pids, process groups and sessions at the end of the command line,
    /// such as `123`, `-123` and `+123`.
    Trailing,
    /// Anything else, such as `--` or `-`.
    Garbage,
}

/// Why a parse of the command line stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Stop {
    /// The command line has an error. After the first parse, procps-ng parses
    /// the command line again with BSD syntax.
    Syntax(SyntaxError),
    /// A check that procps-ng reports as an internal error failed. `ps` exits
    /// at once.
    Internal(InternalError),
    /// An option such as `--help` decides what `ps` does. `ps` does it at
    /// once.
    Exit(Command),
}

impl From<SyntaxError> for Stop {
    fn from(error: SyntaxError) -> Self {
        Self::Syntax(error)
    }
}

impl From<FormatError> for Stop {
    fn from(error: FormatError) -> Self {
        match error {
            FormatError::Syntax(error) => Self::Syntax(error),
            FormatError::Internal(error) => Self::Internal(error),
        }
    }
}

/// What a UNIX option leaves of its argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Rest {
    /// The rest of the argument contains more options.
    Options,
    /// The option consumed the rest of the argument.
    Consumed,
}

/// The long options, in the order of procps-ng's table.
const LONG_OPTIONS: [&str; 37] = [
    "Group",
    "User",
    "cols",
    "columns",
    "context",
    "cumulative",
    "date-format",
    "delimiter",
    "deselect",
    "forest",
    "format",
    "group",
    "header",
    "headers",
    "heading",
    "headings",
    "info",
    "lines",
    "no-header",
    "no-headers",
    "no-heading",
    "no-headings",
    "noheader",
    "noheaders",
    "noheading",
    "noheadings",
    "pid",
    "ppid",
    "quick-pid",
    "rows",
    "sid",
    "signames",
    "sort",
    "tty",
    "user",
    "version",
    "width",
];

/// The selection options of one parse of the command line.
#[derive(Clone, Debug, Default)]
struct Selecting {
    all: bool,
    negate: bool,
    running_only: bool,
    lists: Vec<Listed>,
    simple: SimpleSelect,
}

/// What procps-ng's globals contain during one parse of the command line.
#[derive(Clone, Debug)]
struct State {
    personality: Personality,
    prefer_bsd_defaults: bool,
    screen: Screen,
    selection: Selecting,
    forest: Option<Forest>,
    format_flags: FormatFlags,
    format_modifiers: FormatModifiers,
    header: Header,
    threads: ThreadFlags,
    modifiers: Modifiers,
    date_format: Option<Vec<u8>>,
    delimiter: Option<u8>,
    w_count: u32,
    sort_format: SortFormat,
}

impl State {
    /// The state that procps-ng's `reset_global` gives.
    fn new(personality: Personality, screen: Screen) -> Self {
        Self {
            prefer_bsd_defaults: personality.prefer_bsd_defaults,
            personality,
            screen,
            selection: Selecting::default(),
            forest: None,
            format_flags: FormatFlags::empty(),
            format_modifiers: FormatModifiers::empty(),
            header: Header::Single,
            threads: ThreadFlags::empty(),
            modifiers: Modifiers::empty(),
            date_format: None,
            delimiter: None,
            w_count: 0,
            sort_format: SortFormat::default(),
        }
    }
}

/// What a successful parse gives, apart from the state.
struct Outcome {
    formats: Formats,
    simple: SimpleTests,
}

/// The result of parsing a `ps` command line: the warnings that procps-ng
/// prints while it parses, in order, and what `ps` does.
#[derive(Debug)]
pub(crate) struct Parsed {
    /// The warnings.
    pub(crate) warnings: Vec<Warning>,
    /// What `ps` does, or why it cannot.
    pub(crate) result: Result<Command, PsError>,
}

/// procps-ng's parser, with the state that it keeps between its two parses
/// of the command line.
struct Parser<'a, S> {
    system: &'a S,
    arguments: &'a [OsString],
    position: usize,
    force_bsd: bool,
    state: State,
    first_unknown: Option<OsString>,
    warnings: Vec<Warning>,
}

/// Parses a `ps` command line, `arguments`, which starts with `argv[0]`, as
/// procps-ng's `arg_parse` and `arg_check_conflicts` do.
pub(crate) fn parse(arguments: &[OsString], system: &impl System) -> Parsed {
    let mut warnings = Vec::new();
    let screen = screen(system, &mut warnings);
    let mut parser = Parser {
        system,
        arguments,
        position: 0,
        force_bsd: false,
        state: State::new(Personality::from_environment(system), screen),
        first_unknown: None,
        warnings,
    };

    let result = match parser.arg_parse() {
        Ok(outcome) => parser.command(outcome).map_err(PsError::QuickPid),
        Err(Stop::Syntax(error)) => Err(PsError::Syntax(error)),
        Err(Stop::Internal(error)) => Err(PsError::Internal(error)),
        Err(Stop::Exit(command)) => Ok(command),
    };

    Parsed {
        warnings: parser.warnings,
        result,
    }
}

/// The screen size, as procps-ng's `set_screen_size` decides it: from the
/// terminal, or 80 by 24 without one, with a line as wide as procps-ng's
/// output buffer when standard output is not a terminal, and then from
/// `COLUMNS` and `LINES`. A size that is too small gives a warning.
fn screen(system: &impl System, warnings: &mut Vec<Warning>) -> Screen {
    let window = system.window_size();
    let mut screen = Screen {
        columns: window.map_or(80, |window| i64::from(window.columns)),
        rows: window.map_or(24, |window| i64::from(window.rows)),
    };

    if !system.stdout_is_terminal() {
        screen.columns = OUTBUF_SIZE;
    }

    let size = |name| {
        system
            .variable(name)
            .filter(|value| !value.is_empty())
            .and_then(|value| whole_strtol(value.as_bytes(), Base::Prefixed))
            .filter(|&size| size > 0 && size < OUTBUF_SIZE)
    };

    if let Some(columns) = size("COLUMNS") {
        screen.columns = columns;
    }

    if let Some(rows) = size("LINES") {
        screen.rows = rows;
    }

    if screen.columns < 9 || screen.rows < 2 {
        warnings.push(Warning::ScreenSize(screen));
    }

    screen
}

/// The kind of `argument`, as procps-ng's `arg_type` decides it.
fn kind(argument: &[u8], system: &impl System) -> Kind {
    let letters = system.letters();
    let byte = |index: usize| argument.get(index).copied().unwrap_or_default();

    if letters.is_letter(byte(0)) {
        return Kind::Bsd;
    }

    if byte(0).is_ascii_digit() || byte(0) == b'+' {
        return Kind::Trailing;
    }

    if byte(0) != b'-' {
        return Kind::Garbage;
    }

    if letters.is_letter(byte(1)) {
        return Kind::Unix;
    }

    if byte(1).is_ascii_digit() {
        return Kind::Trailing;
    }

    if byte(1) != b'-' {
        return Kind::Garbage;
    }

    if letters.is_letter(byte(2)) || letters.starts_with_letter(&argument[2..]) {
        return Kind::Gnu;
    }

    Kind::Garbage
}

impl<S: System> Parser<'_, S> {
    /// Resets procps-ng's globals, as its `reset_global`, `reset_parser` and
    /// `reset_sortformat` do.
    fn reset(&mut self) {
        let screen = screen(self.system, &mut self.warnings);

        self.state = State::new(Personality::from_environment(self.system), screen);
        self.position = 0;
    }

    /// Parses the command line, and then again with BSD syntax if the first
    /// parse fails, as procps-ng's `arg_parse` does.
    fn arg_parse(&mut self) -> Result<Outcome, Stop> {
        let force_bsd = self
            .state
            .personality
            .flags
            .contains(personality::Flags::FORCE_BSD);
        let mut first_error = None;

        if !force_bsd {
            match self.pass() {
                Ok(outcome) => return Ok(outcome),
                Err(Stop::Syntax(error)) => first_error = Some(error),
                Err(stop) => return Err(stop),
            }
        }

        self.reset();
        self.force_bsd = true;
        self.state.prefer_bsd_defaults = true;

        let flags = &mut self.state.personality.flags;

        if !flags.intersects(personality::Flags::OLD_M | personality::Flags::BSD_M) {
            flags.insert(personality::Flags::OLD_M);
        }

        self.pass().map_err(|stop| match stop {
            Stop::Syntax(error) => Stop::Syntax(first_error.unwrap_or(error)),
            stop => stop,
        })
    }

    /// One parse of the command line.
    fn pass(&mut self) -> Result<Outcome, Stop> {
        self.parse_all_options()?;
        self.thread_option_check()?;

        let ps_format = self.system.variable("PS_FORMAT");
        let state = &mut self.state;
        let settings = Settings {
            flags: state.format_flags,
            modifiers: state.format_modifiers,
            threads: state.threads,
            forest: state.forest.is_some(),
            personality: &state.personality,
            prefer_bsd_defaults: state.prefer_bsd_defaults,
            ps_format: ps_format.as_deref().map(OsStr::as_bytes),
        };
        let formats =
            state
                .sort_format
                .process(&settings, &mut self.first_unknown, &mut self.warnings)?;
        let simple = selection::select_setup(
            state.selection.simple,
            state.personality.flags,
            state.prefer_bsd_defaults,
            &mut state.selection.all,
        )?;

        if state.w_count > 0 && state.screen.columns < 132 {
            state.screen.columns = 132;
        }

        if state.w_count > 1 {
            state.screen.columns = OUTBUF_SIZE;
        }

        Ok(Outcome { formats, simple })
    }

    /// Reads each argument, as procps-ng's `parse_all_options` does.
    fn parse_all_options(&mut self) -> Result<(), Stop> {
        while self.position + 1 < self.arguments.len() {
            self.position += 1;

            let argument = self.arguments[self.position].as_bytes();

            match kind(argument, self.system) {
                Kind::Gnu => self.parse_gnu_option(argument)?,
                Kind::Unix if !self.force_bsd => self.parse_unix_option(argument)?,
                Kind::Unix => {
                    self.state.prefer_bsd_defaults = true;
                    self.parse_bsd_option(argument)?;
                }
                Kind::Bsd => {
                    let force_bsd = self
                        .state
                        .personality
                        .flags
                        .contains(personality::Flags::FORCE_BSD);

                    if self.force_bsd && !force_bsd {
                        return Err(SyntaxError::WayBad.into());
                    }

                    self.state.prefer_bsd_defaults = true;
                    self.parse_bsd_option(argument)?;
                }
                Kind::Trailing => {
                    self.state.prefer_bsd_defaults = true;
                    self.parse_trailing_pids()?;
                }
                Kind::Garbage => return Err(SyntaxError::Garbage.into()),
            }
        }

        Ok(())
    }

    /// Checks that `option` is the only argument, as procps-ng's `exclusive`
    /// does.
    fn exclusive(&self, option: &'static str) -> Result<(), SyntaxError> {
        if self.arguments.len() == 2 && self.arguments[1].as_bytes() == option.as_bytes() {
            Ok(())
        } else {
            Err(SyntaxError::Exclusive(option))
        }
    }

    /// The next argument, which becomes the current one, unless it is
    /// missing or empty.
    fn next_argument(&mut self) -> Option<&[u8]> {
        let next = self.arguments.get(self.position + 1)?;

        if next.is_empty() {
            return None;
        }

        self.position += 1;
        Some(next.as_bytes())
    }

    /// The argument of the UNIX or BSD option at `index` in `argument`, as
    /// procps-ng's `get_opt_arg` finds it: the rest of the argument, or else
    /// the next argument.
    fn option_argument(&mut self, argument: &[u8], index: usize) -> Option<Vec<u8>> {
        if index + 1 < argument.len() {
            return Some(argument[index + 1..].to_vec());
        }

        self.next_argument().map(<[u8]>::to_vec)
    }

    /// Reads a list of users.
    fn users(&self, list: &[u8]) -> Result<Vec<u32>, SyntaxError> {
        parse_list(list, |user| {
            selection::parse_uid(user, self.state.selection.negate, self.system)
        })
    }

    /// Reads a list of groups.
    fn groups(&self, list: &[u8]) -> Result<Vec<u32>, SyntaxError> {
        parse_list(list, |group| {
            selection::parse_gid(group, self.state.selection.negate, self.system)
        })
    }

    /// Reads a list of terminals.
    fn terminals(&self, list: &[u8]) -> Result<Vec<Terminal>, SyntaxError> {
        parse_list(list, |terminal| selection::parse_tty(terminal, self.system))
    }

    /// Adds a selection list.
    fn select(&mut self, listed: Selector) {
        self.state.selection.lists.push(Listed::Selector(listed));
    }

    /// Reads a cluster of UNIX options, such as `-ef`, as procps-ng's
    /// `parse_sysv_option` does.
    fn parse_unix_option(&mut self, argument: &[u8]) -> Result<(), Stop> {
        for index in 1..argument.len() {
            let option = argument[index];

            if b"CDGOUgopqstu".contains(&option) {
                let value = self
                    .option_argument(argument, index)
                    .ok_or_else(|| unix_missing(option))?;

                if self.unix_option_with_value(option, &value)? == Rest::Consumed {
                    return Ok(());
                }

                continue;
            }

            self.unix_flag(option)?;
        }

        Ok(())
    }

    /// Applies the UNIX option `option`, which takes `value`.
    fn unix_option_with_value(&mut self, option: u8, value: &[u8]) -> Result<Rest, Stop> {
        match option {
            b'C' => {
                let commands = parse_list(value, |name| Ok(selection::parse_command(name)))?;

                self.select(Selector::Commands(commands));
            }
            b'D' => {
                self.state.date_format = Some(value.to_vec());
                return Ok(Rest::Options);
            }
            b'G' => {
                let groups = self.groups(value)?;

                self.select(Selector::RealGroups(groups));
            }
            b'O' => self.state.sort_format.defer(value, Source::UnixCapitalO),
            b'U' => {
                let users = self.users(value)?;

                self.select(Selector::RealUsers(users));
            }
            b'g' => {
                if let Ok(sessions) = parse_list(value, selection::parse_pid) {
                    self.select(Selector::Sessions(sessions));
                } else if let Ok(groups) = self.groups(value) {
                    self.select(Selector::EffectiveGroups(groups));
                } else {
                    return Err(SyntaxError::InvalidSessionsOrGroups.into());
                }
            }
            b'o' => self.state.sort_format.defer(value, Source::UnixO),
            b'p' => {
                let pids = parse_list(value, selection::parse_pid)?;

                self.state.selection.lists.push(Listed::TryQuickPids(pids));
            }
            b'q' => {
                let pids = parse_list(value, selection::parse_pid)?;

                self.select(Selector::QuickPids(pids));
            }
            b's' => {
                let sessions = parse_list(value, selection::parse_pid)?;

                self.select(Selector::Sessions(sessions));
            }
            b't' => {
                let terminals = self.terminals(value)?;

                self.select(Selector::Terminals(terminals));
            }
            _ => {
                let users = self.users(value)?;

                self.select(Selector::EffectiveUsers(users));
            }
        }

        Ok(Rest::Consumed)
    }

    /// Applies the UNIX option `option`, which takes no value.
    fn unix_flag(&mut self, option: u8) -> Result<(), Stop> {
        let state = &mut self.state;

        match option {
            b'A' | b'e' => state.selection.all = true,
            b'F' => {
                state.format_modifiers |= FormatModifiers::F;
                state.format_flags |= FormatFlags::U_F;
                state.modifiers |= Modifiers::FULL;
            }
            b'H' => state.forest = Some(Forest::Unix),
            b'L' => state.threads |= ThreadFlags::U_L,
            b'M' | b'Z' => state.format_modifiers |= FormatModifiers::M,
            b'N' => state.selection.negate = true,
            b'P' => state.format_modifiers |= FormatModifiers::P,
            b'T' => state.threads |= ThreadFlags::U_T,
            b'V' => {
                self.exclusive("-V")?;
                return Err(Stop::Exit(Command::Version));
            }
            b'a' => state.selection.simple |= SimpleSelect::U_A,
            b'c' => state.format_modifiers |= FormatModifiers::C,
            b'd' => state.selection.simple |= SimpleSelect::U_D,
            b'f' => {
                state.format_flags |= FormatFlags::U_F;
                state.modifiers |= Modifiers::FULL;
            }
            b'j' if state.personality.unix.j.is_some() => state.format_flags |= FormatFlags::U_J,
            b'j' => state.format_modifiers |= FormatModifiers::J,
            b'l' => state.format_flags |= FormatFlags::U_L,
            b'm' => state.threads |= ThreadFlags::U_M,
            b'w' => state.w_count += 1,
            b'x' if state.personality.flags.contains(personality::Flags::SVR4_X) => {
                state.format_modifiers |= FormatModifiers::Y;
            }
            b'x' if state.personality.flags.contains(personality::Flags::HPUX_X) => {
                state.w_count += 2;
                state.modifiers |= Modifiers::FULL;
            }
            b'x' => return Err(SyntaxError::PersonalityForX.into()),
            b'y' => state.format_modifiers |= FormatModifiers::Y,
            b'-' => return Err(SyntaxError::DashAmongUnixOptions.into()),
            _ => return Err(SyntaxError::UnknownUnixOption.into()),
        }

        Ok(())
    }

    /// Reads a cluster of BSD options, such as `aux` or, in the second parse,
    /// `-aux`, as procps-ng's `parse_bsd_option` does.
    fn parse_bsd_option(&mut self, argument: &[u8]) -> Result<(), Stop> {
        let start = usize::from(argument.first() == Some(&b'-'));

        for index in start..argument.len() {
            let option = argument[index];

            if option.is_ascii_digit() {
                let pids = parse_list(&argument[index..], selection::parse_pid)?;

                self.select(Selector::Pids(pids));
                return Ok(());
            }

            if option == b't' {
                let terminals = match self.option_argument(argument, index) {
                    Some(value) => self.terminals(&value)?,
                    None => vec![Terminal::Own],
                };

                self.select(Selector::Terminals(terminals));
                return Ok(());
            }

            if b"OUkopq".contains(&option) {
                let value = self
                    .option_argument(argument, index)
                    .ok_or_else(|| bsd_missing(option))?;

                self.bsd_option_with_value(option, &value)?;
                return Ok(());
            }

            self.bsd_flag(option)?;
        }

        Ok(())
    }

    /// Applies the BSD option `option`, which takes `value`.
    fn bsd_option_with_value(&mut self, option: u8, value: &[u8]) -> Result<(), Stop> {
        match option {
            b'O' => self.state.sort_format.defer(value, Source::BsdCapitalO),
            b'U' => {
                let users = self.users(value)?;

                self.select(Selector::EffectiveUsers(users));
            }
            b'k' => self.state.sort_format.defer(value, Source::GnuSort),
            b'o' => self.state.sort_format.defer(value, Source::BsdO),
            b'p' => {
                let pids = parse_list(value, selection::parse_pid)?;

                self.state.selection.lists.push(Listed::TryQuickPids(pids));
            }
            _ => {
                let pids = parse_list(value, selection::parse_pid)?;

                self.select(Selector::QuickPids(pids));
            }
        }

        Ok(())
    }

    /// Applies the BSD option `option`, which takes no value.
    fn bsd_flag(&mut self, option: u8) -> Result<(), Stop> {
        let flags = self.state.personality.flags;
        let state = &mut self.state;

        match option {
            b'H' => state.threads |= ThreadFlags::B_H,
            b'L' => {
                self.exclusive("L")?;
                return Err(Stop::Exit(Command::ListSpecifiers));
            }
            b'S' => state.modifiers |= Modifiers::CUMULATIVE,
            b'T' => state
                .selection
                .lists
                .push(Listed::Selector(Selector::Terminals(vec![Terminal::Own]))),
            b'V' => {
                self.exclusive("V")?;
                return Err(Stop::Exit(Command::Version));
            }
            b'W' => return Err(SyntaxError::ObsoleteW.into()),
            b'X' => state.format_flags |= FormatFlags::L_X,
            b'Z' => state.format_modifiers |= FormatModifiers::M,
            b'a' => state.selection.simple |= SimpleSelect::B_A,
            b'c' => state.modifiers |= Modifiers::COMMAND_NAMES,
            b'e' => state.modifiers |= Modifiers::ENVIRONMENT,
            b'f' => state.forest = Some(Forest::Bsd),
            b'g' => state.selection.simple |= SimpleSelect::B_G,
            b'h' => {
                if state.header != Header::Single {
                    return Err(SyntaxError::TwoHeadingOptions.into());
                }

                state.header = if flags.contains(personality::Flags::BSD_H) {
                    Header::Multiple
                } else {
                    Header::None
                };
            }
            b'j' => state.format_flags |= FormatFlags::B_J,
            b'l' => state.format_flags |= FormatFlags::B_L,
            b'm' if flags.contains(personality::Flags::OLD_M) => {
                state.format_flags |= FormatFlags::L_M;
            }
            b'm' if flags.contains(personality::Flags::BSD_M) => {
                state.sort_format.defer(b"pmem", Source::BsdM);
            }
            b'M' | b'm' => state.threads |= ThreadFlags::B_M,
            b'n' => state.modifiers |= Modifiers::NUMERIC,
            b'r' => state.selection.running_only = true,
            b's' => state.format_flags |= FormatFlags::B_S,
            b'u' => state.format_flags |= FormatFlags::B_U,
            b'v' => state.format_flags |= FormatFlags::B_V,
            b'w' => state.w_count += 1,
            b'x' => state.selection.simple |= SimpleSelect::B_X,
            b'-' => return Err(SyntaxError::DashAmongBsdOptions.into()),
            _ => return Err(SyntaxError::UnknownBsdOption.into()),
        }

        Ok(())
    }

    /// The argument of a long option, as procps-ng's `grab_gnu_arg` finds it:
    /// after a `=` or `:`, or else in the next argument. `rest` is the option
    /// after its name.
    fn gnu_argument(&mut self, rest: &[u8]) -> Option<Vec<u8>> {
        match rest {
            [] => self.next_argument().map(<[u8]>::to_vec),
            [b'=' | b':', value @ ..] if !value.is_empty() => Some(value.to_vec()),
            _ => None,
        }
    }

    /// Reads a long option, such as `--pid=1`, as procps-ng's
    /// `parse_gnu_option` does.
    fn parse_gnu_option(&mut self, argument: &[u8]) -> Result<(), Stop> {
        let option = &argument[2..];
        let length = option
            .iter()
            .position(|&byte| byte == b':' || byte == b'=')
            .unwrap_or(option.len());

        if length > LONG_OPTION_MAX {
            return Err(SyntaxError::UnknownLongOption.into());
        }

        let (name, rest) = option.split_at(length);
        let Some(&name) = LONG_OPTIONS.iter().find(|known| known.as_bytes() == name) else {
            if name == b"help" {
                let section = Section::from_word(self.gnu_argument(rest).as_deref());

                return Err(Stop::Exit(Command::Help(section)));
            }

            return Err(SyntaxError::UnknownLongOption.into());
        };

        match name {
            "Group" | "User" | "group" | "pid" | "ppid" | "quick-pid" | "sid" | "tty" | "user" => {
                let value = self.gnu_argument(rest).ok_or_else(|| gnu_missing(name))?;

                self.gnu_list(name, &value)?;
            }
            "cols" | "columns" | "width" | "lines" | "rows" => {
                let size = self
                    .gnu_argument(rest)
                    .and_then(|value| whole_strtol(&value, Base::Prefixed))
                    .filter(|&size| size > 0 && size < SIZE_LIMIT);

                match (size, name) {
                    (Some(size), "lines" | "rows") => self.state.screen.rows = size,
                    (Some(size), _) => self.state.screen.columns = size,
                    (None, "lines" | "rows") => return Err(SyntaxError::NoRows.into()),
                    (None, _) => return Err(SyntaxError::NoColumns.into()),
                }
            }
            "date-format" | "delimiter" | "format" | "sort" => {
                let value = self.gnu_argument(rest).ok_or_else(|| gnu_missing(name))?;

                self.gnu_value(name, &value)?;
            }
            "info" => {
                self.exclusive("--info")?;

                return Err(Stop::Exit(Command::Info(Box::new(Info {
                    personality: self.state.personality.clone(),
                    screen: self.state.screen,
                }))));
            }
            "version" => {
                self.exclusive("--version")?;
                return Err(Stop::Exit(Command::Version));
            }
            "signames" => self.state.modifiers |= Modifiers::SIGNAL_NAMES,
            "context" => self.state.format_flags |= FormatFlags::F_C,
            _ => self.gnu_flag(name, rest)?,
        }

        Ok(())
    }

    /// Applies the long option `name`, which takes a selection list,
    /// `value`.
    fn gnu_list(&mut self, name: &str, value: &[u8]) -> Result<(), Stop> {
        let listed = match name {
            "Group" => Selector::RealGroups(self.groups(value)?),
            "User" => Selector::RealUsers(self.users(value)?),
            "group" => Selector::EffectiveGroups(self.groups(value)?),
            "pid" => {
                let pids = parse_list(value, selection::parse_pid)?;

                self.state.selection.lists.push(Listed::TryQuickPids(pids));
                return Ok(());
            }
            "ppid" => Selector::Parents(parse_list(value, selection::parse_ppid)?),
            "quick-pid" => Selector::QuickPids(parse_list(value, selection::parse_pid)?),
            "sid" => Selector::Sessions(parse_list(value, selection::parse_pid)?),
            "tty" => Selector::Terminals(self.terminals(value)?),
            _ => Selector::EffectiveUsers(self.users(value)?),
        };

        self.select(listed);
        Ok(())
    }

    /// Applies the long option `name`, which takes `value`.
    fn gnu_value(&mut self, name: &str, value: &[u8]) -> Result<(), Stop> {
        match name {
            "date-format" => self.state.date_format = Some(value.to_vec()),
            "delimiter" => {
                let [delimiter] = value else {
                    return Err(SyntaxError::LongDelimiter.into());
                };

                self.state.delimiter = Some(*delimiter);
            }
            "format" => self.state.sort_format.defer(value, Source::GnuFormat),
            _ => self.state.sort_format.defer(value, Source::GnuSort),
        }

        Ok(())
    }

    /// Applies the long option `name`, which takes no argument. `rest` is the
    /// option after its name.
    fn gnu_flag(&mut self, name: &str, rest: &[u8]) -> Result<(), Stop> {
        let spelling = match name {
            "cumulative" => "--cumulative",
            "deselect" => "--deselect",
            "forest" => "--forest",
            _ if name.starts_with("no") => "--no-heading",
            _ => "--heading",
        };

        if !rest.is_empty() {
            return Err(SyntaxError::UnwantedArgument(spelling).into());
        }

        let state = &mut self.state;

        match spelling {
            "--cumulative" => state.modifiers |= Modifiers::CUMULATIVE,
            "--deselect" => state.selection.negate = true,
            "--forest" => state.forest = Some(Forest::Gnu),
            _ => {
                if state.header != Header::Single {
                    return Err(SyntaxError::TwoHeadingOptions.into());
                }

                state.header = if spelling == "--heading" {
                    Header::Multiple
                } else {
                    Header::None
                };
            }
        }

        Ok(())
    }

    /// Reads the rest of the command line as pids, process groups (`-<pgid>`)
    /// and sessions (`+<sid>`), as procps-ng's `parse_trailing_pids` does.
    fn parse_trailing_pids(&mut self) -> Result<(), Stop> {
        let mut pids = Vec::new();
        let mut process_groups = Vec::new();
        let mut sessions = Vec::new();

        for argument in &self.arguments[self.position..] {
            match argument.as_bytes() {
                [b'-', pid @ ..] => process_groups.push(selection::parse_pid(pid)?),
                [b'+', pid @ ..] => sessions.push(selection::parse_pid(pid)?),
                pid => pids.push(selection::parse_pid(pid)?),
            }
        }

        self.position = self.arguments.len() - 1;

        if !pids.is_empty() {
            self.select(Selector::Pids(pids));
        }

        if !process_groups.is_empty() {
            self.select(Selector::ProcessGroups(process_groups));
        }

        if !sessions.is_empty() {
            self.select(Selector::Sessions(sessions));
        }

        Ok(())
    }

    /// Checks the thread options and decides how threads are shown, as
    /// procps-ng's `thread_option_check` does.
    fn thread_option_check(&mut self) -> Result<(), SyntaxError> {
        let state = &mut self.state;
        let threads = state.threads;

        if threads.is_empty() {
            state.threads = ThreadFlags::SHOW_PROCESS;
            return Ok(());
        }

        if state.forest.is_some() {
            return Err(SyntaxError::ThreadsWithForest);
        }

        let m = ThreadFlags::B_M | ThreadFlags::U_M;

        if threads.contains(ThreadFlags::B_H) && threads.intersects(m) {
            return Err(SyntaxError::HWithM);
        }

        if threads.contains(m) {
            return Err(SyntaxError::BothM);
        }

        if threads.contains(ThreadFlags::U_L | ThreadFlags::U_T) {
            return Err(SyntaxError::LWithT);
        }

        if threads.contains(ThreadFlags::B_H) {
            state.threads |= ThreadFlags::SHOW_PROCESS | ThreadFlags::LOOSE_TASKS;
        }

        if threads.intersects(m) {
            state.threads |=
                ThreadFlags::SHOW_PROCESS | ThreadFlags::SHOW_TASK | ThreadFlags::SHOW_BOTH;
        }

        if threads.intersects(ThreadFlags::U_T | ThreadFlags::U_L) {
            if threads.intersects(m | ThreadFlags::B_H) {
                state.threads |= ThreadFlags::MUST_USE;
            } else {
                state.threads |= ThreadFlags::SHOW_TASK;
            }
        }

        Ok(())
    }

    /// The command for a successful parse, after the checks of procps-ng's
    /// `arg_check_conflicts`.
    fn command(&self, outcome: Outcome) -> Result<Command, QuickPidConflict> {
        let state = &self.state;
        let selection = &state.selection;
        let lists = quick_pids(
            &selection.lists,
            state.forest.is_some(),
            !outcome.formats.sort.is_empty(),
            selection.negate,
        )?;

        Ok(Command::Run(Box::new(Options {
            selection: Selection {
                all: selection.all,
                simple: outcome.simple,
                lists,
                running_only: selection.running_only,
                negate: selection.negate,
            },
            columns: outcome.formats.columns,
            sort: outcome.formats.sort,
            threads: state.threads,
            forest: state.forest,
            header: state.header,
            modifiers: state.modifiers,
            date_format: state.date_format.clone(),
            delimiter: state.delimiter,
            screen: state.screen,
        })))
    }
}

/// Decides whether `-p`, `p` and `--pid` read their processes directly, and
/// checks the uses of `-q`, as procps-ng's `arg_check_conflicts` does.
fn quick_pids(
    listed: &[Listed],
    forest: bool,
    sorted: bool,
    negate: bool,
) -> Result<Vec<Selector>, QuickPidConflict> {
    let count = |quick: fn(&Listed) -> bool| listed.iter().filter(|listed| quick(listed)).count();
    let mut quick = count(|listed| matches!(listed, Listed::Selector(Selector::QuickPids(_))));
    let try_quick = count(|listed| matches!(listed, Listed::TryQuickPids(_)));
    let direct =
        try_quick == 1 && quick == 0 && listed.len() == try_quick && !forest && !sorted && !negate;

    if direct {
        quick += try_quick;
    }

    let lists: Vec<Selector> = listed
        .iter()
        .map(|listed| match listed {
            Listed::Selector(selector) => selector.clone(),
            Listed::TryQuickPids(pids) if direct => Selector::QuickPids(pids.clone()),
            Listed::TryQuickPids(pids) => Selector::Pids(pids.clone()),
        })
        .collect();

    if quick > 1 {
        return Err(QuickPidConflict::Twice);
    }

    if quick == 0 {
        return Ok(lists);
    }

    if lists.len() > quick {
        return Err(QuickPidConflict::Selection);
    }

    if forest {
        return Err(QuickPidConflict::Forest);
    }

    if sorted {
        return Err(QuickPidConflict::Sort);
    }

    if negate {
        return Err(QuickPidConflict::Negation);
    }

    Ok(lists)
}

/// The error for the UNIX option `option` without its argument.
fn unix_missing(option: u8) -> SyntaxError {
    match option {
        b'C' => SyntaxError::NoCommands,
        b'D' => SyntaxError::NoDateFormat("-D"),
        b'G' => SyntaxError::NoRealGroups("-G"),
        b'O' => SyntaxError::NoFormatOrSort("-O"),
        b'U' => SyntaxError::NoRealUsers("-U"),
        b'g' => SyntaxError::NoSessionsOrGroups,
        b'o' => SyntaxError::NoFormat("-o"),
        b'p' => SyntaxError::NoPids("-p"),
        b'q' => SyntaxError::NoQuickPids("-q"),
        b's' => SyntaxError::NoSessions,
        b't' => SyntaxError::NoTerminals,
        _ => SyntaxError::NoUsers("-u"),
    }
}

/// The error for the BSD option `option` without its argument.
fn bsd_missing(option: u8) -> SyntaxError {
    match option {
        b'O' => SyntaxError::NoFormatOrSort("O"),
        b'U' => SyntaxError::NoUsers("U"),
        b'k' => SyntaxError::NoSort("'k'"),
        b'o' => SyntaxError::NoFormat("o"),
        b'p' => SyntaxError::NoPids("p"),
        _ => SyntaxError::NoQuickPids("q"),
    }
}

/// The error for the long option `name` without its argument.
fn gnu_missing(name: &str) -> SyntaxError {
    match name {
        "Group" => SyntaxError::NoRealGroups("--Group"),
        "User" => SyntaxError::NoRealUsers("--User"),
        "date-format" => SyntaxError::NoDateFormat("--date-format"),
        "delimiter" => SyntaxError::NoDelimiter,
        "format" => SyntaxError::NoFormat("--format"),
        "group" => SyntaxError::NoEffectiveGroups,
        "pid" => SyntaxError::NoPids("--pid"),
        "ppid" => SyntaxError::NoPids("--ppid"),
        "quick-pid" => SyntaxError::NoQuickPids("--quick-pid"),
        "sid" => SyntaxError::NoSids,
        "sort" => SyntaxError::NoSort("--sort"),
        "tty" => SyntaxError::NoTtys,
        _ => SyntaxError::NoEffectiveUsers,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, ffi::OsString};

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Parsed, parse};
    use crate::ps::{
        error::{InternalError, PsError, QuickPidConflict, SyntaxError, Warning},
        format::{Column, Content, Order, SortKey},
        help::Section,
        options::{Command, Forest, Header, Info, Modifiers, Options, Screen, ThreadFlags},
        personality::Personality,
        selection::{Selection, Selector, SimpleTests, Terminal},
        specifier,
        system::{Letters, WindowSize, testing::FakeSystem},
    };

    /// A parse's warnings and result, in a form that tests can compare.
    #[derive(Debug, Eq, PartialEq)]
    enum Outcome {
        Command(Command),
        Syntax(SyntaxError),
        QuickPid(QuickPidConflict),
        Internal(InternalError),
    }

    fn system(variables: &[(&'static str, &'static str)]) -> FakeSystem {
        FakeSystem {
            variables: HashMap::from_iter(variables.iter().copied()),
            ..FakeSystem::default()
        }
    }

    fn outcome(parsed: Parsed) -> (Vec<Warning>, Outcome) {
        let outcome = match parsed.result {
            Ok(command) => Outcome::Command(command),
            Err(PsError::Syntax(error)) => Outcome::Syntax(error),
            Err(PsError::QuickPid(error)) => Outcome::QuickPid(error),
            Err(PsError::Internal(error)) => Outcome::Internal(error),
            Err(error) => panic!("the parser returned {error:?}"),
        };

        (parsed.warnings, outcome)
    }

    fn parsed_with(line: &[&str], system: &FakeSystem) -> (Vec<Warning>, Outcome) {
        let arguments: Vec<OsString> = ["ps"].iter().chain(line).map(OsString::from).collect();

        outcome(parse(&arguments, system))
    }

    fn parsed(
        line: &[&str],
        variables: &[(&'static str, &'static str)],
    ) -> (Vec<Warning>, Outcome) {
        parsed_with(line, &system(variables))
    }

    /// The columns of the specifiers `names`, with their default headers and
    /// widths.
    fn columns(names: &[&str]) -> Vec<Column> {
        names
            .iter()
            .map(|name| {
                let specifier = specifier::find(name.as_bytes()).expect("the name is a specifier");

                Column {
                    content: Content::Specifier(specifier),
                    header: specifier.header.as_bytes().to_vec(),
                    width: specifier.width(),
                }
            })
            .collect()
    }

    /// The options of a command line without options, apart from the
    /// selection and the columns.
    fn options(selection: Selection, columns: Vec<Column>) -> Options {
        Options {
            selection,
            columns,
            sort: Vec::new(),
            threads: ThreadFlags::SHOW_PROCESS,
            forest: None,
            header: Header::Single,
            modifiers: Modifiers::empty(),
            date_format: None,
            delimiter: None,
            screen: Screen {
                columns: 131_072,
                rows: 24,
            },
        }
    }

    fn selection(simple: SimpleTests, lists: Vec<Selector>) -> Selection {
        Selection {
            all: false,
            simple,
            lists,
            running_only: false,
            negate: false,
        }
    }

    fn all() -> Selection {
        Selection {
            all: true,
            ..selection(SimpleTests::empty(), Vec::new())
        }
    }

    fn run(options: Options) -> (Vec<Warning>, Outcome) {
        (
            Vec::new(),
            Outcome::Command(Command::Run(Box::new(options))),
        )
    }

    const UNIX_DEFAULT: [&str; 4] = ["pid", "tname", "time", "ucmd"];
    const BSD_DEFAULT: [&str; 5] = ["pid", "tname", "stat", "bsdtime", "args"];
    const BSD_USER: [&str; 11] = [
        "user",
        "pid",
        "pcpu",
        "pmem",
        "vsz",
        "rss",
        "tname",
        "stat",
        "start_time",
        "bsdtime",
        "args",
    ];

    #[test]
    fn without_options_ps_lists_the_user_s_processes_on_its_terminal() {
        assert_eq!(
            parsed(&[], &[]),
            run(options(
                selection(
                    SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL,
                    Vec::new()
                ),
                columns(&UNIX_DEFAULT)
            ))
        );
    }

    #[test]
    fn a_dash_before_bsd_options_is_read_as_bsd_syntax() {
        assert_eq!(
            parsed(&["-aux"], &[]),
            run(options(all(), columns(&BSD_USER)))
        );
    }

    #[test]
    fn unix_options_give_the_full_format() {
        assert_eq!(
            parsed(&["-ef"], &[]),
            run(Options {
                modifiers: Modifiers::FULL,
                ..options(
                    Selection {
                        all: true,
                        ..selection(
                            SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL,
                            Vec::new()
                        )
                    },
                    columns(&[
                        "uid_hack", "pid", "ppid", "c", "stime", "tname", "time", "cmd"
                    ])
                )
            })
        );
    }

    #[test]
    fn a_failed_unix_option_makes_ps_use_bsd_syntax() {
        assert_eq!(
            parsed(&["-x"], &[]),
            run(options(
                selection(SimpleTests::GIVEN | SimpleTests::OWN_USER, Vec::new()),
                columns(&BSD_DEFAULT)
            ))
        );
    }

    #[test]
    fn a_single_pid_list_reads_its_processes_directly() {
        let mut columns = columns(&["pid", "comm"]);

        for column in &mut columns {
            column.header.clear();
        }

        assert_eq!(
            parsed(&["-p", "1", "-o", "pid=,comm="], &[]),
            run(options(
                selection(
                    SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL,
                    vec![Selector::QuickPids(vec![1])]
                ),
                columns
            ))
        );
    }

    #[test]
    fn lists_and_bsd_selections_are_kept_in_command_line_order() {
        assert_eq!(
            parsed(
                &["--ppid", "1", "T", "-p", "2", "--sid=3", "4", "-5", "+6"],
                &[]
            ),
            run(options(
                selection(
                    SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL,
                    vec![
                        Selector::Parents(vec![1]),
                        Selector::Terminals(vec![Terminal::Own]),
                        Selector::Pids(vec![2]),
                        Selector::Sessions(vec![3]),
                        Selector::Pids(vec![4]),
                        Selector::ProcessGroups(vec![5]),
                        Selector::Sessions(vec![6]),
                    ]
                ),
                columns(&BSD_DEFAULT)
            ))
        );
    }

    #[test]
    fn bsd_modifiers_and_long_options_are_recorded() {
        let ppid = specifier::find(b"ppid").expect("ppid is a specifier");

        assert_eq!(
            parsed(
                &[
                    "cenS",
                    "--sort=-ppid",
                    "--forest",
                    "--no-headers",
                    "--signames",
                    "--delimiter=|",
                    "--date-format",
                    "%F",
                    "--rows",
                    "40",
                ],
                &[]
            ),
            run(Options {
                sort: vec![SortKey {
                    specifier: ppid,
                    order: Order::Descending,
                }],
                forest: Some(Forest::Gnu),
                header: Header::None,
                modifiers: Modifiers::COMMAND_NAMES
                    | Modifiers::ENVIRONMENT
                    | Modifiers::NUMERIC
                    | Modifiers::CUMULATIVE
                    | Modifiers::SIGNAL_NAMES,
                date_format: Some(b"%F".to_vec()),
                delimiter: Some(b'|'),
                screen: Screen {
                    columns: 131_072,
                    rows: 40,
                },
                ..options(
                    selection(
                        SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL,
                        Vec::new()
                    ),
                    columns(&BSD_DEFAULT)
                )
            })
        );
    }

    /// A system whose standard output is a terminal of 80 columns and 30
    /// rows.
    fn terminal() -> FakeSystem {
        FakeSystem {
            window: Some(WindowSize {
                columns: 80,
                rows: 30,
            }),
            ..FakeSystem::default()
        }
    }

    /// `options` on the terminal of [`terminal`], with `columns` columns.
    fn on_terminal(options: Options, columns: i64) -> Options {
        Options {
            screen: Screen { columns, rows: 30 },
            ..options
        }
    }

    fn unix_default() -> Options {
        options(
            selection(
                SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL,
                Vec::new(),
            ),
            columns(&UNIX_DEFAULT),
        )
    }

    fn bsd_default() -> Options {
        options(
            selection(
                SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL,
                Vec::new(),
            ),
            columns(&BSD_DEFAULT),
        )
    }

    #[rstest]
    #[case::w(&["w"], &[], on_terminal(bsd_default(), 132))]
    #[case::ww(&["ww"], &[], on_terminal(bsd_default(), 131_072))]
    #[case::w_after_cols(&["--cols=200", "w"], &[], on_terminal(bsd_default(), 200))]
    #[case::cols_hexadecimal(&["--width=0x20"], &[], on_terminal(unix_default(), 32))]
    #[case::cols_binary(&["--cols", "0b1010"], &[], on_terminal(unix_default(), 10))]
    #[case::largest_cols(&["--cols", "1999999999"], &[], on_terminal(unix_default(), 1_999_999_999))]
    #[case::columns_variable(&[], &[("COLUMNS", "131071")], on_terminal(unix_default(), 131_071))]
    #[case::columns_variable_too_large(&[], &[("COLUMNS", "131072")], on_terminal(unix_default(), 80))]
    #[case::hpux_x(
        &["-x"],
        &[("PS_PERSONALITY", "hpux")],
        on_terminal(Options { modifiers: Modifiers::FULL, ..unix_default() }, 131_072)
    )]
    fn the_screen_size_comes_from_the_terminal_and_the_options(
        #[case] line: &[&str],
        #[case] variables: &[(&'static str, &'static str)],
        #[case] expected: Options,
    ) {
        let system = FakeSystem {
            variables: HashMap::from_iter(variables.iter().copied()),
            ..terminal()
        };

        assert_eq!(parsed_with(line, &system), run(expected));
    }

    fn key(name: &str, order: Order) -> SortKey {
        SortKey {
            specifier: specifier::find(name.as_bytes()).expect("the name is a specifier"),
            order,
        }
    }

    /// The column of IRIX's `:` between `SZ` and `RSS`.
    fn colon() -> Column {
        Column {
            content: Content::Text,
            header: b":".to_vec(),
            width: 1,
        }
    }

    fn unix(lists: Vec<Selector>) -> Selection {
        selection(SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL, lists)
    }

    fn bsd(lists: Vec<Selector>) -> Selection {
        selection(SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL, lists)
    }

    #[rstest]
    #[case::pids_with_sort(
        &["-p", "1", "--sort", "pid"],
        &[],
        Options {
            sort: vec![key("pid", Order::Ascending)],
            ..options(unix(vec![Selector::Pids(vec![1])]), columns(&UNIX_DEFAULT))
        }
    )]
    #[case::pids_with_forest(
        &["-p", "1", "f"],
        &[],
        Options {
            forest: Some(Forest::Bsd),
            ..options(bsd(vec![Selector::Pids(vec![1])]), columns(&BSD_DEFAULT))
        }
    )]
    #[case::pids_with_negation(
        &["-p", "1", "-N"],
        &[],
        options(
            Selection { negate: true, ..unix(vec![Selector::Pids(vec![1])]) },
            columns(&UNIX_DEFAULT)
        )
    )]
    #[case::binary_pid(&["-p", "0b11"], &[], options(unix(vec![Selector::QuickPids(vec![3])]), columns(&UNIX_DEFAULT)))]
    #[case::sessions_before_groups(&["-g", "20"], &[], options(unix(vec![Selector::Sessions(vec![20])]), columns(&UNIX_DEFAULT)))]
    #[case::groups(&["-g", "staff"], &[], options(unix(vec![Selector::EffectiveGroups(vec![20])]), columns(&UNIX_DEFAULT)))]
    #[case::trailing_pid(&["1"], &[], options(bsd(vec![Selector::Pids(vec![1])]), columns(&BSD_DEFAULT)))]
    #[case::own_terminal(&["t"], &[], options(bsd(vec![Selector::Terminals(vec![Terminal::Own])]), columns(&BSD_DEFAULT)))]
    #[case::terminal_under_pts(&["-t", "3"], &[], options(unix(vec![Selector::Terminals(vec![Terminal::Device(0x1000_0003)])]), columns(&UNIX_DEFAULT)))]
    #[case::commands(
        &["-C", "a,b"],
        &[],
        options(unix(vec![Selector::Commands(vec![b"a".to_vec(), b"b".to_vec()])]), columns(&UNIX_DEFAULT))
    )]
    #[case::long_command(
        &["-C", "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijkl"],
        &[],
        options(
            unix(vec![Selector::Commands(vec![b"abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijk".to_vec()])]),
            columns(&UNIX_DEFAULT)
        )
    )]
    #[case::extra_full(
        &["-F"],
        &[],
        Options {
            modifiers: Modifiers::FULL,
            ..options(
                unix(Vec::new()),
                columns(&["uid_hack", "pid", "ppid", "c", "sz", "rss", "psr", "stime", "tname", "time", "cmd"])
            )
        }
    )]
    #[case::running(
        &["r"],
        &[],
        options(Selection { running_only: true, ..bsd(Vec::new()) }, columns(&BSD_DEFAULT))
    )]
    #[case::y_with_bsd_long(
        &["-y", "l"],
        &[],
        options(
            bsd(Vec::new()),
            columns(&["uid", "pid", "ppid", "priority", "nice", "vsz", "rss", "wchan", "stat", "tname", "bsdtime", "args"])
        )
    )]
    #[case::threads_with_user_format(
        &["-m", "-L", "u"],
        &[],
        Options {
            threads: ThreadFlags::U_M
                | ThreadFlags::U_L
                | ThreadFlags::SHOW_PROCESS
                | ThreadFlags::SHOW_TASK
                | ThreadFlags::SHOW_BOTH
                | ThreadFlags::MUST_USE,
            ..options(
                bsd(Vec::new()),
                columns(&[
                    "user", "pid", "lwp", "pcpu", "nlwp", "pmem", "vsz", "rss", "tname", "stat",
                    "start_time", "bsdtime", "args",
                ])
            )
        }
    )]
    #[case::label_before_jobs(
        &["j", "-M"],
        &[],
        options(
            bsd(Vec::new()),
            columns(&["label", "ppid", "pid", "pgid", "sid", "tname", "tpgid", "stat", "uid", "bsdtime", "args"])
        )
    )]
    #[case::irix_long(
        &["-l"],
        &[("PS_PERSONALITY", "irix")],
        options(
            unix(Vec::new()),
            [
                columns(&["f", "s", "uid", "pid", "ppid", "c", "priority", "ni", "sgi_p", "sz"]),
                vec![colon()],
                columns(&["sgi_rss", "wchan", "tname", "time", "ucmd"]),
            ]
            .concat()
        )
    )]
    #[case::linux_long(
        &["-l"],
        &[("PS_PERSONALITY", "linux")],
        options(
            unix(Vec::new()),
            columns(&["f", "s", "uid", "pid", "ppid", "c", "opri", "ni", "sgi_p", "sz", "wchan", "tname", "time", "ucmd"])
        )
    )]
    #[case::linux_full(
        &["-f"],
        &[("PS_PERSONALITY", "linux")],
        Options {
            modifiers: Modifiers::FULL,
            ..options(unix(Vec::new()), columns(&["user", "pid", "ppid", "c", "stime", "tname", "time", "cmd"]))
        }
    )]
    #[case::aix_user(
        &["u"],
        &[("PS_PERSONALITY", "aix")],
        options(
            bsd(Vec::new()),
            columns(&["user", "pid", "pcpu", "pmem", "vsz", "rss", "tt", "stat", "start", "time", "command"])
        )
    )]
    #[case::svr4_x(
        &["-x"],
        &[("PS_PERSONALITY", "svr4")],
        options(selection(SimpleTests::GIVEN | SimpleTests::OWN_USER, Vec::new()), columns(&BSD_DEFAULT))
    )]
    fn a_command_line_gives_procps_ng_s_options(
        #[case] line: &[&str],
        #[case] variables: &[(&'static str, &'static str)],
        #[case] expected: Options,
    ) {
        assert_eq!(parsed(line, variables), run(expected));
    }

    #[rstest]
    #[case::version(&["-V"], Command::Version)]
    #[case::bsd_version(&["V"], Command::Version)]
    #[case::long_version(&["--version"], Command::Version)]
    #[case::specifiers(&["L"], Command::ListSpecifiers)]
    #[case::help(&["--help"], Command::Help(Section::Summary))]
    #[case::help_section(&["--help", "all"], Command::Help(Section::All))]
    #[case::help_attached(&["--help=t"], Command::Help(Section::Threads))]
    #[case::help_takes_the_next_argument(&["--help", "-e"], Command::Help(Section::Summary))]
    #[case::help_without_value(&["--help="], Command::Help(Section::Summary))]
    #[case::help_after_bsd_retry(&["-x", "--help"], Command::Help(Section::Summary))]
    #[case::help_before_error(&["--help", "s", "-y"], Command::Help(Section::Simple))]
    fn an_option_can_end_the_parse(#[case] line: &[&str], #[case] expected: Command) {
        assert_eq!(parsed(line, &[]), (Vec::new(), Outcome::Command(expected)));
    }

    #[test]
    fn info_reports_the_personality_and_the_screen() {
        let variables = [("PS_PERSONALITY", "sunos4")];

        assert_eq!(
            parsed(&["--info"], &variables),
            (
                Vec::new(),
                Outcome::Command(Command::Info(Box::new(Info {
                    personality: Personality::from_environment(&system(&variables)),
                    screen: Screen {
                        columns: 131_072,
                        rows: 24,
                    },
                })))
            )
        );
    }

    #[rstest]
    #[case::exclusive(&["-eV"], &[], SyntaxError::Exclusive("-V"))]
    #[case::exclusive_with_bsd(&["-V", "x"], &[], SyntaxError::Exclusive("-V"))]
    #[case::exclusive_list(&["L", "x"], &[], SyntaxError::Exclusive("L"))]
    #[case::exclusive_info(&["--cols", "12", "--info"], &[], SyntaxError::Exclusive("--info"))]
    #[case::missing_format(&["-o"], &[], SyntaxError::NoFormat("-o"))]
    #[case::missing_pids(&["-p"], &[], SyntaxError::NoPids("-p"))]
    #[case::empty_argument(&["-p", ""], &[], SyntaxError::NoPids("-p"))]
    #[case::unknown_long(&["--foo"], &[], SyntaxError::UnknownLongOption)]
    #[case::abbreviated_long(&["--hel"], &[], SyntaxError::UnknownLongOption)]
    #[case::too_long(&["--abcdefghijklmnop"], &[], SyntaxError::UnknownLongOption)]
    #[case::unknown_specifier(&["-o", "foo"], &[], SyntaxError::UnknownSpecifier(OsString::from("foo")))]
    #[case::first_parse_wins(&["-x", "-o"], &[], SyntaxError::PersonalityForX)]
    #[case::format_after_x(&["-o", "foo", "-x"], &[], SyntaxError::PersonalityForX)]
    #[case::bsd_after_failure(&["a", "-x"], &[], SyntaxError::PersonalityForX)]
    #[case::bsd_personality(&["-o"], &[("PS_PERSONALITY", "bsd")], SyntaxError::NoFormat("o"))]
    #[case::y_without_l(&["-y"], &[], SyntaxError::YWithoutL)]
    #[case::date_continues(&["-Dabc"], &[], SyntaxError::UnknownUnixOption)]
    #[case::pid_syntax(&["-p", "08"], &[], SyntaxError::PidSyntax)]
    #[case::pid_blank(&["-p", " 5"], &[], SyntaxError::ImproperList)]
    #[case::pid_range(&["-p", "-1"], &[], SyntaxError::PidRange)]
    #[case::trailing_dash(&["1", "-e"], &[], SyntaxError::PidSyntax)]
    #[case::lone_dash(&["-"], &[], SyntaxError::Garbage)]
    #[case::lone_plus(&["+"], &[], SyntaxError::PidRange)]
    #[case::double_dash(&["--"], &[], SyntaxError::Garbage)]
    #[case::triple_dash(&["---x"], &[], SyntaxError::Garbage)]
    #[case::non_ascii_long(&["--é"], &[], SyntaxError::Garbage)]
    #[case::unknown_user(&["-u", "x"], &[], SyntaxError::UnknownUser)]
    #[case::user_before_negation(&["-u", "x", "-N"], &[], SyntaxError::UnknownUser)]
    #[case::user_range(&["-u", "4294967295"], &[], SyntaxError::UserRange)]
    #[case::group_range(&["-G", "4294967295"], &[], SyntaxError::GroupRange)]
    #[case::sessions_or_groups(&["-g", "x,1x"], &[], SyntaxError::InvalidSessionsOrGroups)]
    #[case::terminal(&["-t", "x"], &[], SyntaxError::TerminalNotFound)]
    #[case::terminal_path(&["-t", "/etc/passwd"], &[], SyntaxError::NotATerminal)]
    #[case::terminal_after_capital_f(&["-Ftx"], &[], SyntaxError::TerminalNotFound)]
    #[case::columns(&["--cols=0"], &[], SyntaxError::NoColumns)]
    #[case::rows(&["--rows", "0"], &[], SyntaxError::NoRows)]
    #[case::cumulative(&["--cumulative=x"], &[], SyntaxError::UnwantedArgument("--cumulative"))]
    #[case::no_heading(&["--noheader=x"], &[], SyntaxError::UnwantedArgument("--no-heading"))]
    #[case::heading(&["--headings=x"], &[], SyntaxError::UnwantedArgument("--heading"))]
    #[case::delimiter(&["--delimiter", "ab"], &[], SyntaxError::LongDelimiter)]
    #[case::empty_delimiter(&["--delimiter="], &[], SyntaxError::NoDelimiter)]
    #[case::sort(&["--sort"], &[], SyntaxError::NoSort("--sort"))]
    #[case::k(&["k"], &[], SyntaxError::NoSort("'k'"))]
    #[case::forest_threads(&["-L", "f"], &[], SyntaxError::ThreadsWithForest)]
    #[case::h_and_m(&["H", "m"], &[], SyntaxError::HWithM)]
    #[case::both_m(&["m", "-m"], &[], SyntaxError::BothM)]
    #[case::l_and_t(&["-L", "-T"], &[], SyntaxError::LWithT)]
    #[case::threads_with_list(&["-L", "m", "-o", "pid"], &[], SyntaxError::ThreadsWithFormatList)]
    #[case::threads_with_ps_format(&["-L", "m"], &[("PS_FORMAT", "pid")], SyntaxError::ThreadsWithPsFormat)]
    #[case::selection(&["-d", "x"], &[], SyntaxError::SelectionConflict)]
    #[case::headings(&["--headers", "--no-headers"], &[], SyntaxError::TwoHeadingOptions)]
    #[case::modifier_with_list(&["-j", "-o", "pid"], &[], SyntaxError::ModifierWithFormatList)]
    #[case::conflicting(&["-l", "u"], &[], SyntaxError::ConflictingFormats)]
    #[case::late_capital_o(&["-o", "comm", "-O", "pid"], &[], SyntaxError::LateCapitalO)]
    #[case::sort_codes(&["Opp"], &[], SyntaxError::BadSortCode)]
    #[case::parent_sort(&["OP", "f"], &[], SyntaxError::ParentSortWithForest)]
    #[case::multiple_sorts(&["--sort", "pid", "Op"], &[], SyntaxError::MultipleSorts)]
    #[case::w(&["W"], &[], SyntaxError::ObsoleteW)]
    #[case::unknown_unix(&["-E"], &[], SyntaxError::UnknownUnixOption)]
    #[case::improper_commands(&["-C", ",a"], &[], SyntaxError::ImproperList)]
    #[case::too_many_columns(&["--cols", "2000000000"], &[], SyntaxError::NoColumns)]
    #[case::cpu_and_space_before_aix(&["-o", "%cpu %p"], &[], SyntaxError::UnknownSpecifier(OsString::from("%p")))]
    #[case::binary_session(&["-s", "0b0"], &[], SyntaxError::PidRange)]
    fn a_bad_command_line_has_procps_ng_s_error(
        #[case] line: &[&str],
        #[case] variables: &[(&'static str, &'static str)],
        #[case] expected: SyntaxError,
    ) {
        assert_eq!(
            parsed(line, variables),
            (Vec::new(), Outcome::Syntax(expected))
        );
    }

    #[test]
    fn a_letter_after_two_dashes_depends_on_the_locale() {
        let system = FakeSystem {
            letters: Letters::for_codeset("UTF-8"),
            ..FakeSystem::default()
        };

        assert_eq!(
            parsed_with(&["--é"], &system),
            (Vec::new(), Outcome::Syntax(SyntaxError::UnknownLongOption))
        );
    }

    #[test]
    fn an_unknown_user_after_negation_is_minus_1() {
        assert_eq!(
            parsed(&["-N", "-u", "x"], &[]),
            run(Options {
                selection: Selection {
                    negate: true,
                    ..selection(
                        SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL,
                        vec![Selector::EffectiveUsers(vec![u32::MAX])]
                    )
                },
                ..options(all(), columns(&UNIX_DEFAULT))
            })
        );
    }

    #[rstest]
    #[case::twice(&["-q", "1", "-q", "2"], QuickPidConflict::Twice)]
    #[case::selection(&["-q", "1", "-p", "2"], QuickPidConflict::Selection)]
    #[case::forest(&["-q", "1", "f"], QuickPidConflict::Forest)]
    #[case::sort(&["-q", "1", "--sort", "pid"], QuickPidConflict::Sort)]
    #[case::negation(&["-q", "1", "-N"], QuickPidConflict::Negation)]
    fn quick_pids_cannot_be_combined(#[case] line: &[&str], #[case] expected: QuickPidConflict) {
        assert_eq!(parsed(line, &[]), (Vec::new(), Outcome::QuickPid(expected)));
    }

    #[test]
    fn two_pid_lists_are_not_read_directly() {
        assert_eq!(
            parsed(&["-p", "1", "-p", "2"], &[]),
            run(options(
                selection(
                    SimpleTests::OWN_USER | SimpleTests::OWN_TERMINAL,
                    vec![Selector::Pids(vec![1]), Selector::Pids(vec![2])]
                ),
                columns(&UNIX_DEFAULT)
            ))
        );
    }

    #[test]
    fn a_missing_column_for_a_modifier_is_an_internal_error() {
        assert_eq!(
            parsed(&["-c", "u"], &[]),
            (Vec::new(), Outcome::Internal(InternalError::NoPriForC))
        );
    }

    #[test]
    fn each_parse_warns_about_a_small_screen() {
        let screen = Screen {
            columns: 5,
            rows: 24,
        };

        assert_eq!(
            parsed(&["-x", "--help", "s"], &[("COLUMNS", "5")]),
            (
                vec![Warning::ScreenSize(screen), Warning::ScreenSize(screen)],
                Outcome::Command(Command::Help(Section::Simple))
            )
        );
    }

    #[test]
    fn an_ignored_ps_format_reports_the_first_unknown_specifier() {
        let pid = specifier::find(b"pid").expect("pid is a specifier");

        assert_eq!(
            parsed(&["-Op"], &[("PS_FORMAT", "bar")]),
            (
                vec![Warning::PsFormatIgnored(SyntaxError::UnknownSpecifier(
                    OsString::from("p")
                ))],
                Outcome::Command(Command::Run(Box::new(Options {
                    sort: vec![SortKey {
                        specifier: pid,
                        order: Order::Ascending,
                    }],
                    ..options(
                        selection(
                            SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL,
                            Vec::new()
                        ),
                        columns(&BSD_DEFAULT)
                    )
                })))
            )
        );
    }

    #[test]
    fn the_bsd_personality_reads_unix_options_as_bsd_options() {
        assert_eq!(
            parsed(&["-ef"], &[("PS_PERSONALITY", "bsd")]),
            run(Options {
                forest: Some(Forest::Bsd),
                modifiers: Modifiers::ENVIRONMENT,
                ..options(
                    selection(
                        SimpleTests::OWN_USER | SimpleTests::HAS_TERMINAL,
                        Vec::new()
                    ),
                    columns(&BSD_DEFAULT)
                )
            })
        );
    }
}
