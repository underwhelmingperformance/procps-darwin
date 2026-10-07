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
    collections::HashSet,
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use clap::{Arg, ArgAction, ArgMatches, value_parser};
use darwin_proc::{CaseFolding, Gid, Locale, Uid};

use crate::{
    c_number::atoi,
    pgrep::{
        Mode, PgrepError,
        error::LinuxOption,
        getopt::{self, Stop},
        pidfile,
        table::{FlagId, OPTIONS, OptionId, Takes, ValueId},
    },
    signal::{self, Form, LinuxSignal, Lookup},
};

/// The namespaces that procps-ng's `--nslist` accepts.
const NAMESPACES: [&str; 8] = ["cgroup", "ipc", "mnt", "net", "pid", "time", "user", "uts"];

/// What the command line asks the tool to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// Print the usage text on standard output and exit with status 0.
    Help,
    /// Print the version and exit with status 0.
    Version,
    /// Select processes as the [`Options`] describe.
    Run(Box<Options>),
}

/// Which of the selected processes the tool keeps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Pick {
    /// Every process that matches.
    #[default]
    All,
    /// Only the process that started most recently (`-n`).
    Newest,
    /// Only the process that started least recently (`-o`).
    Oldest,
}

/// Which tasks the tool selects and lists.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Tasks {
    /// Processes.
    #[default]
    Processes,
    /// Each thread of each process (`-w`).
    Threads,
}

/// How the pattern is matched.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Matching {
    /// `-f`: match the full command line.
    pub(crate) full: bool,
    /// `-i`: match without regard to case.
    pub(crate) ignore_case: bool,
    /// `-x`: match the whole name or command line.
    pub(crate) exact: bool,
}

/// The criteria other than the pattern. Each list keeps the values in the
/// order of the command line, and a later occurrence of an option replaces the
/// list of an earlier one, as in procps-ng.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Filters {
    /// `-G`: real group IDs.
    pub(crate) real_groups: Option<Vec<i64>>,
    /// `-p`, or the pid from the pid file of `-F`: process IDs.
    pub(crate) pids: Option<Vec<i64>>,
    /// `-P`: parent process IDs.
    pub(crate) parents: Option<Vec<i64>>,
    /// `-U`: real user IDs.
    pub(crate) real_users: Option<Vec<i64>>,
    /// `-u`: effective user IDs.
    pub(crate) effective_users: Option<Vec<i64>>,
    /// `-g`: process group IDs, with 0 replaced by the tool's own process
    /// group.
    pub(crate) process_groups: Option<Vec<i64>>,
    /// `-s`: session IDs, with 0 replaced by the tool's own session.
    pub(crate) sessions: Option<Vec<i64>>,
    /// `-t`: terminal names.
    pub(crate) terminals: Option<Vec<OsString>>,
    /// `--env`: `NAME` or `NAME=value` entries.
    pub(crate) environment: Option<Vec<OsString>>,
    /// `-r`: the process states to select.
    pub(crate) states: Option<OsString>,
    /// `-O`: the age in seconds that a process must reach. procps-ng ignores
    /// an age of 0, so the field is `None` for it.
    pub(crate) older: Option<libc::c_int>,
    /// `-H`: select only processes that catch the signal.
    pub(crate) require_handler: bool,
    /// `-A`: leave out the tool's ancestors.
    pub(crate) ignore_ancestors: bool,
}

/// What the tool prints for each selected task.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Listing {
    /// The task's ID.
    #[default]
    Ids,
    /// The ID and the name (`-l`).
    Names,
    /// The ID and the command line (`-a`, which wins over `-l`).
    CommandLines,
}

/// What `pgrep` prints about the selected tasks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Report {
    /// Each task, separated by the delimiter.
    List(Listing),
    /// The number of selected tasks (`-c`, which wins over `--quiet`).
    Count,
    /// Nothing (`--quiet`).
    Quiet,
}

impl Default for Report {
    fn default() -> Self {
        Self::List(Listing::default())
    }
}

/// How the tool prints what it selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Output {
    /// What `pgrep` prints.
    pub(crate) report: Report,
    /// `-d`: the delimiter between tasks.
    pub(crate) delimiter: OsString,
    /// `-Q`: quote names and command lines for the shell.
    pub(crate) shell_quote: bool,
    /// `-e`: print each process that `pkill` signals.
    pub(crate) echo: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            report: Report::default(),
            delimiter: OsString::from("\n"),
            shell_quote: false,
            echo: false,
        }
    }
}

/// The options of a `pgrep` or `pkill` command line, after procps-ng's checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    /// The signal that `pkill` sends.
    pub(crate) signal: libc::c_int,
    /// The pattern.
    pub(crate) pattern: Option<OsString>,
    /// How the pattern is matched.
    pub(crate) matching: Matching,
    /// `-v`: select the tasks that do not match.
    pub(crate) inverse: bool,
    /// `-n` or `-o`.
    pub(crate) pick: Pick,
    /// `-w`: whether the tool selects processes or threads.
    pub(crate) tasks: Tasks,
    /// The criteria other than the pattern.
    pub(crate) filters: Filters,
    /// How the tool prints what it selected.
    pub(crate) output: Output,
    /// The first option for a Linux concept that is still in effect after
    /// procps-ng's checks. The tools refuse it where procps-ng would start to
    /// select processes.
    pub(crate) unsupported: Option<LinuxOption>,
    /// Whether the pid came from a pid file that the tool opened by its path,
    /// which procps-ng keeps open. A pid file of `-` is standard input.
    pub(crate) opened_pid_file: bool,
}

/// What [`Options::parse`] reads from outside the command line.
pub(crate) trait Inputs {
    /// Whether `POSIXLY_CORRECT` is set, which makes `getopt_long` stop at the
    /// first operand.
    fn posixly_correct(&self) -> bool;

    /// The pid at the start of the pid file `path`, which must be locked by
    /// another process if `lock` is set.
    fn read_pidfile(&self, path: &Path, lock: bool) -> Result<i64, PgrepError>;

    /// The tool's own process group, which `-g 0` selects.
    fn process_group(&self) -> i64;

    /// The tool's own session, which `-s 0` selects.
    fn session(&self) -> i64;

    /// The user ID of the user called `name`, if there is one.
    fn user_id(&self, name: &OsStr) -> Option<u32>;

    /// The group ID of the group called `name`, if there is one.
    fn group_id(&self, name: &OsStr) -> Option<u32>;

    /// How the locale folds case, which decides how signal names compare.
    fn case_folding(&self) -> CaseFolding;
}

/// The inputs of the running tool: its environment, process group and session,
/// the file system, and the user and group databases.
struct LiveInputs;

impl Inputs for LiveInputs {
    fn posixly_correct(&self) -> bool {
        std::env::var_os("POSIXLY_CORRECT").is_some()
    }

    fn read_pidfile(&self, path: &Path, lock: bool) -> Result<i64, PgrepError> {
        pidfile::read(path, lock)
    }

    fn process_group(&self) -> i64 {
        i64::from(rustix::process::getpgrp().as_raw_nonzero().get())
    }

    fn session(&self) -> i64 {
        rustix::process::getsid(None).map_or(0, |session| i64::from(session.as_raw_nonzero().get()))
    }

    fn user_id(&self, name: &OsStr) -> Option<u32> {
        Uid::from_name(name).ok().flatten().map(Uid::as_raw)
    }

    fn group_id(&self, name: &OsStr) -> Option<u32> {
        Gid::from_name(name).ok().flatten().map(Gid::as_raw)
    }

    fn case_folding(&self) -> CaseFolding {
        Locale::current().case_folding()
    }
}

impl Options {
    /// The options before procps-ng reads the command line.
    fn initial(signal: libc::c_int) -> Self {
        Self {
            signal,
            pattern: None,
            matching: Matching::default(),
            inverse: false,
            pick: Pick::All,
            tasks: Tasks::Processes,
            filters: Filters::default(),
            output: Output::default(),
            unsupported: None,
            opened_pid_file: false,
        }
    }

    /// Reads the command line `arguments`, which start with the program name.
    ///
    /// A pid file that `-F` gives is read here, at the point where procps-ng
    /// reads it: after the checks of `-L` without `-F` and of `-F` with `-p`,
    /// and before the checks of `--quiet` with `-a` or `-l` and of the number
    /// of patterns. An error in reading the pid file therefore comes before the
    /// errors of those later checks.
    ///
    /// ```
    /// use assert_matches::assert_matches;
    /// use procps::pgrep::{Command, Mode, Options};
    ///
    /// let command = Options::parse(Mode::Pgrep, ["pgrep", "-c", "sshd"].map(Into::into))?;
    ///
    /// assert_matches!(command, Command::Run(_));
    /// # Ok::<(), procps::pgrep::PgrepError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`PgrepError`] if the command line is not valid, with the
    /// message and exit status that procps-ng gives.
    pub fn parse(
        mode: Mode,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Command, PgrepError> {
        Self::parse_with(mode, arguments, &LiveInputs)
    }

    /// Reads the command line `arguments`, as [`Options::parse`] does, with
    /// `inputs` in place of the live process.
    pub(crate) fn parse_with(
        mode: Mode,
        arguments: impl IntoIterator<Item = OsString>,
        inputs: &impl Inputs,
    ) -> Result<Command, PgrepError> {
        let mut arguments: Vec<OsString> = arguments.into_iter().collect();
        let mut signal = libc::SIGTERM;
        let mut signal_option_unsupported = None;

        if mode == Mode::Pkill {
            match take_signal_option(&mut arguments, inputs.case_folding()) {
                Some(Ok(number)) => signal = number,
                Some(Err(linux)) => signal_option_unsupported = Some(LinuxOption::Signal(linux)),
                None => {}
            }
        }

        let program = arguments.first().cloned().unwrap_or_default();
        let mut arguments: Vec<OsString> = std::iter::once(program.clone())
            .chain(getopt::detach_equals_values(
                mode,
                arguments.get(1..).unwrap_or_default(),
            ))
            .collect();
        let require_order = inputs.posixly_correct();

        // `clap` reads options after operands too. With `POSIXLY_CORRECT`,
        // insert `--` before the first operand so that `clap` stops there, as
        // `getopt_long` does.
        if require_order
            && let Stop::Operand(index) =
                getopt::scan(mode, arguments.get(1..).unwrap_or_default(), true)
        {
            arguments.insert(index + 1, OsString::from("--"));
        }

        let command = command(mode);
        let mut replay = Replay::new(mode, signal, inputs);
        replay.unsupported.extend(signal_option_unsupported);

        let error = match command.clone().try_get_matches_from(&arguments) {
            Ok(matches) => return replay.run(&matches),
            Err(error) => error,
        };

        let Stop::Error(found) =
            getopt::scan(mode, arguments.get(1..).unwrap_or_default(), require_order)
        else {
            tracing::warn!(%error, "clap rejected a command line that getopt_long accepts");
            return Err(PgrepError::Usage);
        };

        // `getopt_long` returns each option before the error to procps-ng's
        // loop, which handles it before `getopt_long` reports the error. One
        // of those options can end the loop or fail.
        let before = std::iter::once(program.clone()).chain(found.before);

        if let Ok(matches) = command.try_get_matches_from(before)
            && let Some(command) = replay.run_loop(&matches)?
        {
            return Ok(command);
        }

        Err(PgrepError::Getopt {
            program,
            message: found.message,
        })
    }
}

/// Removes the first argument of the form `-<signal>` and returns the signal,
/// as procps-ng's `signal_option` does for `pkill`. On Linux, procps-ng
/// accepts the signals that Darwin lacks, so their arguments are removed too,
/// and `-rtmin` is not read as `-r tmin`. Such a signal is returned as the
/// error. The caller refuses it after procps-ng's checks, unless a `--signal`
/// replaces it.
fn take_signal_option(
    arguments: &mut Vec<OsString>,
    folding: CaseFolding,
) -> Option<Result<libc::c_int, LinuxSignal>> {
    let (index, signal) = arguments
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, argument)| {
            let name = argument.as_bytes().strip_prefix(b"-")?;

            match signal::lookup(OsStr::from_bytes(name), Form::Option, folding) {
                Lookup::Signal(number) => Some((index, Ok(number))),
                Lookup::LinuxOnly(signal) => Some((index, Err(signal))),
                Lookup::Unknown => None,
            }
        })?;

    arguments.remove(index);

    Some(signal)
}

/// The `clap` command for the tool that `mode` selects, built from
/// [`OPTIONS`].
fn command(mode: Mode) -> clap::Command {
    let options = OPTIONS.iter().map(|spec| {
        // The replay needs the position of every occurrence. `Count` keeps
        // only the last position, and `SetTrue` rejects a repeated flag.
        let mut argument = Arg::new(spec.clap_id()).action(ArgAction::Append);

        if let Some(long) = spec.long {
            argument = argument.long(long);
        }

        if let Some(short) = spec.short(mode) {
            argument = argument.short(char::from(short));
        }

        match spec.takes() {
            Takes::Argument => argument
                .allow_hyphen_values(true)
                .value_parser(value_parser!(OsString)),
            Takes::Nothing => argument
                .num_args(0)
                .default_missing_value("true")
                .value_parser(value_parser!(bool)),
        }
    });

    clap::Command::new(mode.name())
        .disable_help_flag(true)
        .disable_version_flag(true)
        .infer_long_args(true)
        .args(options)
        .arg(
            Arg::new(PATTERNS)
                .num_args(1..)
                .action(ArgAction::Append)
                .value_parser(value_parser!(OsString)),
        )
}

/// The ID of the `clap` argument for the patterns.
const PATTERNS: &str = "patterns";

/// One option on the command line.
#[derive(Clone, Copy, Debug)]
enum Occurrence<'a> {
    Flag(FlagId),
    Value(ValueId, &'a OsStr),
}

/// procps-ng's option loop, run over the occurrences in command-line order.
struct Replay<'a, I> {
    mode: Mode,
    inputs: &'a I,
    options: Options,
    pidfile: Option<PathBuf>,
    lock: bool,
    criteria: usize,
    /// The options among `-c`, `--quiet`, `-l` and `-a` that the command line
    /// gives. procps-ng checks their combinations after the option loop.
    reports: HashSet<FlagId>,
    /// The options for Linux concepts that are still in effect, in
    /// command-line order. A `-<signal>` comes first, because procps-ng
    /// applies it before the option loop.
    unsupported: Vec<LinuxOption>,
}

impl<'a, I: Inputs> Replay<'a, I> {
    fn new(mode: Mode, signal: libc::c_int, inputs: &'a I) -> Self {
        Self {
            mode,
            inputs,
            options: Options::initial(signal),
            pidfile: None,
            lock: false,
            criteria: 0,
            reports: HashSet::new(),
            unsupported: Vec::new(),
        }
    }

    /// Runs procps-ng's option loop, then its checks of the result.
    fn run(mut self, matches: &ArgMatches) -> Result<Command, PgrepError> {
        if let Some(command) = self.run_loop(matches)? {
            return Ok(command);
        }

        let patterns: Vec<&OsString> = matches
            .get_many::<OsString>(PATTERNS)
            .into_iter()
            .flatten()
            .collect();

        if self.lock && self.pidfile.is_none() {
            return Err(PgrepError::LockWithoutPidfile);
        }

        if let Some(pidfile) = &self.pidfile {
            if self.options.filters.pids.is_some() {
                return Err(PgrepError::PidfileAndPid);
            }

            self.options.filters.pids = Some(vec![self.inputs.read_pidfile(pidfile, self.lock)?]);
            self.options.opened_pid_file = pidfile.as_os_str() != "-";
        }

        let given = |id| self.reports.contains(&id);

        if given(FlagId::Quiet) && given(FlagId::ListFull) {
            return Err(PgrepError::QuietAndListFull);
        }

        if given(FlagId::Quiet) && given(FlagId::ListName) {
            return Err(PgrepError::QuietAndListName);
        }

        let report = if given(FlagId::Count) {
            Report::Count
        } else if given(FlagId::Quiet) {
            Report::Quiet
        } else if given(FlagId::ListFull) {
            Report::List(Listing::CommandLines)
        } else if given(FlagId::ListName) {
            Report::List(Listing::Names)
        } else {
            Report::List(Listing::Ids)
        };
        let options = &mut self.options;
        options.output.report = report;

        match patterns.as_slice() {
            [pattern] => options.pattern = Some((*pattern).clone()),
            [] if self.criteria == 0 => return Err(PgrepError::NoCriteria),
            [] => {}
            _ => return Err(PgrepError::TwoPatterns),
        }

        // The signal matters to `pgrep` only for `-H`.
        if self.mode == Mode::Pgrep && !self.options.filters.require_handler {
            self.withdraw(|option| matches!(option, LinuxOption::Signal(_)));
        }

        self.options.unsupported = self.unsupported.first().copied();

        Ok(Command::Run(Box::new(self.options)))
    }

    /// Runs procps-ng's option loop over every occurrence. Returns the
    /// command that ends the loop early, such as [`Command::Help`] for
    /// `--help`, or `None` if the loop finishes.
    fn run_loop(&mut self, matches: &ArgMatches) -> Result<Option<Command>, PgrepError> {
        for occurrence in occurrences(matches) {
            if let Some(command) = self.apply(occurrence)? {
                return Ok(Some(command));
            }
        }

        Ok(None)
    }

    /// Records that the command line uses `option`, an option for a Linux
    /// concept. After procps-ng's checks, which follow the option loop, the
    /// first option for a Linux concept that is still in effect becomes
    /// [`Options::unsupported`].
    fn unsupported(&mut self, option: LinuxOption) {
        if !self.unsupported.contains(&option) {
            self.unsupported.push(option);
        }
    }

    /// Removes the options for which `which` returns true from the options for
    /// Linux concepts.
    ///
    /// Callers withdraw an option when a later occurrence replaces it, or when
    /// `pgrep` ignores the signal because `-H` is absent.
    fn withdraw(&mut self, which: impl Fn(&LinuxOption) -> bool) {
        self.unsupported.retain(|option| !which(option));
    }

    /// Applies one occurrence, as one pass of procps-ng's `switch` does.
    /// Returns the command that ends the loop, such as [`Command::Help`] for
    /// `--help`.
    fn apply(&mut self, occurrence: Occurrence<'_>) -> Result<Option<Command>, PgrepError> {
        match occurrence {
            Occurrence::Flag(id) => self.apply_flag(id),
            Occurrence::Value(id, value) => {
                self.apply_value(id, value)?;
                Ok(None)
            }
        }
    }

    /// Applies an option that takes no argument.
    fn apply_flag(&mut self, id: FlagId) -> Result<Option<Command>, PgrepError> {
        let options = &mut self.options;

        match id {
            FlagId::Echo => options.output.echo = true,
            FlagId::Logpidfile => self.lock = true,
            FlagId::Version => return Ok(Some(Command::Version)),
            FlagId::Count | FlagId::ListName | FlagId::ListFull | FlagId::Quiet => {
                self.reports.insert(id);
            }
            FlagId::Full => options.matching.full = true,
            FlagId::IgnoreCase if options.matching.ignore_case => return Ok(Some(Command::Help)),
            FlagId::IgnoreCase => options.matching.ignore_case = true,
            FlagId::IgnoreAncestors => options.filters.ignore_ancestors = true,
            FlagId::Newest | FlagId::Oldest | FlagId::Inverse
                if options.pick != Pick::All || options.inverse =>
            {
                return Err(PgrepError::Usage);
            }
            FlagId::Newest => {
                options.pick = Pick::Newest;
                self.criteria += 1;
            }
            FlagId::Oldest => {
                options.pick = Pick::Oldest;
                self.criteria += 1;
            }
            FlagId::Inverse => options.inverse = true,
            FlagId::Lightweight => options.tasks = Tasks::Threads,
            FlagId::Exact => options.matching.exact = true,
            FlagId::RequireHandler => {
                options.filters.require_handler = true;
                self.criteria += 1;
            }
            // `pgrep` accepts `--mrelease` and ignores it, as procps-ng does,
            // because the option only changes what `pkill` does.
            FlagId::Mrelease if self.mode == Mode::Pgrep => {}
            FlagId::Mrelease => self.unsupported(LinuxOption::Mrelease),
            FlagId::ShellQuote => options.output.shell_quote = true,
            FlagId::Help => return Ok(Some(Command::Help)),
            FlagId::Question => return Err(PgrepError::Usage),
        }

        Ok(None)
    }

    /// Applies an option that takes the argument `value`.
    fn apply_value(&mut self, id: ValueId, value: &OsStr) -> Result<(), PgrepError> {
        let options = &mut self.options;
        let inputs = self.inputs;
        let text = |text: &OsStr| Ok(text.to_owned());

        match id {
            ValueId::Signal => {
                let signal = signal_value(value, inputs.case_folding())?;
                let replaced = |option: &LinuxOption| matches!(option, LinuxOption::Signal(_));

                match signal {
                    Ok(number) => {
                        self.options.signal = number;
                        self.withdraw(replaced);
                    }
                    Err(linux) => {
                        self.withdraw(replaced);
                        self.unsupported(LinuxOption::Signal(linux));
                    }
                }
            }
            ValueId::Pidfile => self.pidfile = Some(PathBuf::from(value)),
            ValueId::Group => {
                options.filters.real_groups = Some(list(value, |text| group_id(inputs, text))?);
            }
            ValueId::Pid => options.filters.pids = Some(list(value, number)?),
            ValueId::Parent => options.filters.parents = Some(list(value, number)?),
            ValueId::Uid => {
                options.filters.real_users = Some(list(value, |text| user_id(inputs, text))?);
            }
            ValueId::Delimiter => value.clone_into(&mut options.output.delimiter),
            ValueId::Pgroup => {
                options.filters.process_groups =
                    Some(list(value, |text| process_group(inputs, text))?);
            }
            ValueId::Older => {
                options.filters.older = Some(atoi(value.as_bytes())).filter(|&older| older != 0);
            }
            ValueId::Session => {
                options.filters.sessions = Some(list(value, |text| session(inputs, text))?);
            }
            ValueId::Terminal => options.filters.terminals = Some(list(value, text)?),
            ValueId::Euid => {
                options.filters.effective_users = Some(list(value, |text| user_id(inputs, text))?);
            }
            // When `atoi` reads the pid as 0, `--ns` also sets `-r`'s run states,
            // because the `if` in procps-ng's `--ns` case contains `-r`'s case
            // label. procps-ng then reads no namespaces, so only the run states
            // take effect.
            ValueId::Ns if atoi(value.as_bytes()) == 0 => {
                options.filters.states = Some(value.to_owned());
                self.withdraw(|option| *option == LinuxOption::Ns);
            }
            ValueId::Ns => self.unsupported(LinuxOption::Ns),
            ValueId::Runstates => options.filters.states = Some(value.to_owned()),
            ValueId::Nslist => {
                list(value, namespace)?;
                self.unsupported(LinuxOption::Nslist);
            }
            // `pgrep` accepts `--queue` and ignores it, as procps-ng does,
            // because the option only changes what `pkill` does.
            ValueId::Queue if self.mode == Mode::Pgrep => {}
            ValueId::Queue => self.unsupported(LinuxOption::Queue),
            ValueId::Cgroup => {
                list(value, text)?;
                self.unsupported(LinuxOption::Cgroup);
            }
            ValueId::Env => options.filters.environment = Some(list(value, text)?),
        }

        // Every option with an argument counts as a matching criterion, except
        // these.
        if !matches!(
            id,
            ValueId::Signal | ValueId::Delimiter | ValueId::Nslist | ValueId::Queue
        ) {
            self.criteria += 1;
        }

        Ok(())
    }
}

/// Every option on the command line, in command-line order.
fn occurrences(matches: &ArgMatches) -> Vec<Occurrence<'_>> {
    let mut occurrences: Vec<(usize, Occurrence<'_>)> = Vec::new();

    for spec in &OPTIONS {
        let id = spec.clap_id();

        if matches.value_source(id) != Some(clap::parser::ValueSource::CommandLine) {
            continue;
        }

        let indices = matches.indices_of(id).into_iter().flatten();

        match spec.id {
            OptionId::Value(id) => occurrences.extend(
                indices
                    .zip(
                        matches
                            .get_many::<OsString>(spec.clap_id())
                            .into_iter()
                            .flatten(),
                    )
                    .map(|(index, value)| (index, Occurrence::Value(id, value))),
            ),
            OptionId::Flag(id) => {
                occurrences.extend(indices.map(|index| (index, Occurrence::Flag(id))));
            }
        }
    }

    occurrences.sort_by_key(|&(index, _)| index);

    occurrences
        .into_iter()
        .map(|(_, occurrence)| occurrence)
        .collect()
}

/// The signal for `--signal`: a signal name or number. Otherwise, a value
/// that starts with a digit is converted as `atoi` converts it. A signal that
/// Darwin lacks, including a real-time signal number from `atoi`, is returned
/// as the inner error.
fn signal_value(
    value: &OsStr,
    folding: CaseFolding,
) -> Result<Result<libc::c_int, LinuxSignal>, PgrepError> {
    let lookup = match signal::lookup(value, Form::Value, folding) {
        Lookup::Unknown if value.as_bytes().first().is_some_and(u8::is_ascii_digit) => {
            signal::from_number(atoi(value.as_bytes()))
        }
        lookup => lookup,
    };

    match lookup {
        Lookup::Signal(number) => Ok(Ok(number)),
        Lookup::LinuxOnly(signal) => Ok(Err(signal)),
        Lookup::Unknown => Err(PgrepError::UnknownSignal(value.to_owned())),
    }
}

/// The elements of a comma-separated `value`, each converted by `convert`, as
/// procps-ng's `split_list` reads them. Empty elements are skipped. A list of
/// only commas is [`PgrepError::EmptyList`], and an empty value is
/// [`PgrepError::Usage`].
fn list<T>(
    value: &OsStr,
    convert: impl Fn(&OsStr) -> Result<T, PgrepError>,
) -> Result<Vec<T>, PgrepError> {
    let mut rest = value.as_bytes();
    let mut elements = Vec::new();

    while !rest.is_empty() {
        if elements.is_empty() && rest == b"," {
            return Err(PgrepError::EmptyList);
        }

        let separator = rest.iter().position(|&byte| byte == b',');

        if separator == Some(0) {
            rest = &rest[1..];
            continue;
        }

        let end = separator.unwrap_or(rest.len());
        elements.push(convert(OsStr::from_bytes(&rest[..end]))?);
        rest = rest.get(end + 1..).unwrap_or_default();
    }

    if elements.is_empty() {
        return Err(PgrepError::Usage);
    }

    Ok(elements)
}

/// A number as procps-ng's `strict_atol` reads it: an optional sign and only
/// decimal digits. A sign alone, or the empty string, reads as 0. Like
/// `strict_atol`, it rejects some numbers below `i64::MAX`, from
/// 9223372036854775800 upwards.
fn strict_number(value: &OsStr) -> Option<i64> {
    let bytes = value.as_bytes();
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let mut magnitude: i64 = 0;

    for &byte in digits {
        if !byte.is_ascii_digit() || magnitude >= i64::MAX / 10 {
            return None;
        }

        let digit = i64::from(byte - b'0');
        magnitude *= 10;

        if magnitude >= i64::MAX - digit {
            return None;
        }

        magnitude += digit;
    }

    Some(if negative { -magnitude } else { magnitude })
}

fn number(value: &OsStr) -> Result<i64, PgrepError> {
    strict_number(value).ok_or_else(|| PgrepError::NotANumber(value.to_owned()))
}

fn user_id(inputs: &impl Inputs, value: &OsStr) -> Result<i64, PgrepError> {
    if let Some(id) = strict_number(value) {
        return Ok(id);
    }

    inputs
        .user_id(value)
        .map(i64::from)
        .ok_or_else(|| PgrepError::UnknownUser(value.to_owned()))
}

fn group_id(inputs: &impl Inputs, value: &OsStr) -> Result<i64, PgrepError> {
    if let Some(id) = strict_number(value) {
        return Ok(id);
    }

    inputs
        .group_id(value)
        .map(i64::from)
        .ok_or_else(|| PgrepError::UnknownGroup(value.to_owned()))
}

fn process_group(inputs: &impl Inputs, value: &OsStr) -> Result<i64, PgrepError> {
    match strict_number(value) {
        Some(0) => Ok(inputs.process_group()),
        Some(id) => Ok(id),
        None => Err(PgrepError::InvalidProcessGroup(value.to_owned())),
    }
}

fn session(inputs: &impl Inputs, value: &OsStr) -> Result<i64, PgrepError> {
    match strict_number(value) {
        Some(0) => Ok(inputs.session()),
        Some(id) => Ok(id),
        None => Err(PgrepError::InvalidSession(value.to_owned())),
    }
}

/// A namespace name, which procps-ng's `conv_ns` checks.
fn namespace(value: &OsStr) -> Result<(), PgrepError> {
    if NAMESPACES
        .iter()
        .any(|known| known.as_bytes() == value.as_bytes())
    {
        return Ok(());
    }

    Err(PgrepError::UnknownNamespace)
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        os::unix::ffi::OsStrExt,
        path::Path,
    };

    use assert_matches::assert_matches;
    use darwin_proc::CaseFolding;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{
        Command, Filters, Inputs, Listing, Matching, Options, Output, Pick, Report, Tasks, command,
        list, strict_number,
    };
    use crate::pgrep::{
        LinuxOption, LinuxSignal, Mode, PgrepError,
        getopt::{self, Stop},
    };

    /// Inputs with a fixed `POSIXLY_CORRECT`, a pid file that contains `pid`,
    /// or is not valid if `pid` is `None`, and a locale that folds case as
    /// `case_folding` does.
    struct FakeInputs {
        posixly_correct: bool,
        pid: Option<i64>,
        case_folding: CaseFolding,
    }

    impl Inputs for FakeInputs {
        fn posixly_correct(&self) -> bool {
            self.posixly_correct
        }

        fn read_pidfile(&self, _path: &Path, _lock: bool) -> Result<i64, PgrepError> {
            self.pid.ok_or(PgrepError::InvalidPidfile)
        }

        fn process_group(&self) -> i64 {
            4242
        }

        fn session(&self) -> i64 {
            4343
        }

        fn user_id(&self, name: &OsStr) -> Option<u32> {
            (name == "alice").then_some(501)
        }

        fn group_id(&self, name: &OsStr) -> Option<u32> {
            (name == "staff").then_some(20)
        }

        fn case_folding(&self) -> CaseFolding {
            self.case_folding
        }
    }

    #[test]
    fn names_and_zeros_are_resolved_through_the_inputs() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(
                Mode::Pgrep,
                &[
                    "pgrep", "-g", "0,7", "-s", "0", "-u", "alice,0", "-U", "alice", "-G", "staff"
                ],
            )?,
            Options {
                filters: Filters {
                    process_groups: Some(vec![4242, 7]),
                    sessions: Some(vec![4343]),
                    effective_users: Some(vec![501, 0]),
                    real_users: Some(vec![501]),
                    real_groups: Some(vec![20]),
                    ..Filters::default()
                },
                ..Options::initial(libc::SIGTERM)
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::user(&["pgrep", "-u", "bob"], "invalid user name: bob")]
    #[case::group(&["pgrep", "-G", "wheelz"], "invalid group name: wheelz")]
    fn an_unknown_name_is_an_error(#[case] arguments: &[&str], #[case] expected: &str) {
        assert_matches!(
            parse(Mode::Pgrep, arguments),
            Err(error @ (PgrepError::UnknownUser(_) | PgrepError::UnknownGroup(_)))
                if error.to_string() == expected
        );
    }

    /// An error in a form that tests can compare. procps-ng prints no message
    /// for [`PgrepError::Usage`] or [`PgrepError::UnknownNamespace`], so those
    /// errors are compared by variant, and every other error by its message.
    #[derive(Debug, PartialEq, Eq)]
    enum Failure {
        Usage,
        UnknownNamespace,
        Message(String),
    }

    impl From<PgrepError> for Failure {
        fn from(error: PgrepError) -> Self {
            match error {
                PgrepError::Usage => Self::Usage,
                PgrepError::UnknownNamespace => Self::UnknownNamespace,
                other => Self::Message(other.to_string()),
            }
        }
    }

    fn message(text: &str) -> Failure {
        Failure::Message(text.to_owned())
    }

    const INPUTS: FakeInputs = FakeInputs {
        posixly_correct: false,
        pid: Some(5),
        case_folding: CaseFolding::Ascii,
    };

    fn parse_with(
        mode: Mode,
        arguments: &[&str],
        inputs: &FakeInputs,
    ) -> Result<Command, PgrepError> {
        Options::parse_with(mode, arguments.iter().map(OsString::from), inputs)
    }

    fn parse(mode: Mode, arguments: &[&str]) -> Result<Command, PgrepError> {
        parse_with(mode, arguments, &INPUTS)
    }

    fn options(mode: Mode, arguments: &[&str]) -> Result<Options, Box<dyn std::error::Error>> {
        match parse(mode, arguments)? {
            Command::Run(options) => Ok(*options),
            other => Err(format!("expected options, got {other:?}").into()),
        }
    }

    /// The options of a command line that has only the pattern `x`.
    fn with_pattern() -> Options {
        Options {
            pattern: Some(OsString::from("x")),
            ..Options::initial(libc::SIGTERM)
        }
    }

    #[rstest]
    #[case::pids(&[], Report::List(Listing::Ids))]
    #[case::names(&["-l"], Report::List(Listing::Names))]
    #[case::command_lines_over_names(&["-l", "-a"], Report::List(Listing::CommandLines))]
    #[case::quiet(&["--quiet"], Report::Quiet)]
    #[case::count_over_quiet(&["--quiet", "-c"], Report::Count)]
    #[case::count_over_names(&["-c", "-l"], Report::Count)]
    fn the_report_follows_procps_ng_s_precedence(
        #[case] flags: &[&str],
        #[case] report: Report,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let arguments: Vec<&str> = ["pgrep"]
            .iter()
            .chain(flags)
            .chain(&["x"])
            .copied()
            .collect();

        assert_eq!(
            options(Mode::Pgrep, &arguments)?,
            Options {
                output: Output {
                    report,
                    ..Output::default()
                },
                ..with_pattern()
            }
        );

        Ok(())
    }

    // procps-ng counts most selection options as a matching criterion, so
    // pgrep runs with one of them and no pattern. The delimiter, the signal,
    // the namespace types of `--nslist` and pkill's queue value do not count.
    // Nor do the flags that change how the tool matches or prints, such as
    // `-A`, `-x` and `-l`, but `-n`, `-o` and `-H` count.
    #[rstest]
    #[case::delimiter(Mode::Pgrep, &["pgrep", "-d", "x"], false)]
    #[case::signal(Mode::Pgrep, &["pgrep", "--signal", "9"], false)]
    #[case::namespace_list(Mode::Pgrep, &["pgrep", "--nslist", "net"], false)]
    #[case::queue(Mode::Pkill, &["pkill", "-q", "1"], false)]
    #[case::ignore_ancestors(Mode::Pgrep, &["pgrep", "-A"], false)]
    #[case::exact(Mode::Pgrep, &["pgrep", "-x"], false)]
    #[case::echo(Mode::Pkill, &["pkill", "-e"], false)]
    #[case::count(Mode::Pgrep, &["pgrep", "-c"], false)]
    #[case::full(Mode::Pgrep, &["pgrep", "-f"], false)]
    #[case::ignore_case(Mode::Pgrep, &["pgrep", "-i"], false)]
    #[case::list_name(Mode::Pgrep, &["pgrep", "-l"], false)]
    #[case::list_full(Mode::Pgrep, &["pgrep", "-a"], false)]
    #[case::inverse(Mode::Pgrep, &["pgrep", "-v"], false)]
    #[case::lightweight(Mode::Pgrep, &["pgrep", "-w"], false)]
    #[case::quiet(Mode::Pgrep, &["pgrep", "--quiet"], false)]
    #[case::shell_quote(Mode::Pgrep, &["pgrep", "-Q"], false)]
    #[case::newest(Mode::Pgrep, &["pgrep", "-n"], true)]
    #[case::oldest(Mode::Pgrep, &["pgrep", "-o"], true)]
    #[case::require_handler(Mode::Pgrep, &["pgrep", "-H"], true)]
    #[case::older(Mode::Pgrep, &["pgrep", "-O", "0"], true)]
    #[case::run_states(Mode::Pgrep, &["pgrep", "-r", "S"], true)]
    #[case::environment(Mode::Pgrep, &["pgrep", "--env", "A"], true)]
    fn an_option_counts_as_a_criterion_as_in_procps_ng(
        #[case] mode: Mode,
        #[case] arguments: &[&str],
        #[case] counts: bool,
    ) {
        let parsed = parse(mode, arguments);

        if counts {
            assert_matches!(parsed, Ok(Command::Run(_)));
        } else {
            assert_matches!(parsed, Err(PgrepError::NoCriteria));
        }
    }

    #[test]
    fn every_selection_option_is_read() -> Result<(), Box<dyn std::error::Error>> {
        let options = options(
            Mode::Pgrep,
            &[
                "pgrep", "-c", "-f", "-i", "-x", "-l", "-a", "-w", "-Q", "-d", ",", "-u", "0,1",
                "-U", "2", "-G", "3", "-p", "4,,5", "-P", "6", "-t", "ttys000", "-O", "7x", "-s",
                "8", "-g", "9", "--env", "A=1,B", "-r", "S", "-A", "-H", "-n", "pattern",
            ],
        )?;

        assert_eq!(
            options,
            Options {
                signal: libc::SIGTERM,
                pattern: Some(OsString::from("pattern")),
                matching: Matching {
                    full: true,
                    ignore_case: true,
                    exact: true,
                },
                inverse: false,
                pick: Pick::Newest,
                tasks: Tasks::Threads,
                filters: Filters {
                    real_groups: Some(vec![3]),
                    pids: Some(vec![4, 5]),
                    parents: Some(vec![6]),
                    real_users: Some(vec![2]),
                    effective_users: Some(vec![0, 1]),
                    process_groups: Some(vec![9]),
                    sessions: Some(vec![8]),
                    terminals: Some(vec![OsString::from("ttys000")]),
                    environment: Some(vec![OsString::from("A=1"), OsString::from("B")]),
                    states: Some(OsString::from("S")),
                    older: Some(7),
                    require_handler: true,
                    ignore_ancestors: true,
                },
                output: Output {
                    report: Report::Count,
                    delimiter: OsString::from(","),
                    shell_quote: true,
                    echo: false,
                },
                unsupported: None,
                opened_pid_file: false,
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::first(&["pkill", "-9", "x"], libc::SIGKILL)]
    #[case::after_the_pattern(&["pkill", "x", "-HUP"], libc::SIGHUP)]
    #[case::negative_zero(&["pkill", "--0", "x"], 0)]
    #[case::vertical_tab(&["pkill", "-\x0b9", "x"], libc::SIGKILL)]
    #[case::long_name(&["pkill", "--signal", "SIGUSR1", "x"], libc::SIGUSR1)]
    #[case::long_negative_zero(&["pkill", "--signal=-0", "x"], 0)]
    #[case::long_number(&["pkill", "--signal", "99", "x"], 99)]
    #[case::number_with_letters(&["pkill", "--signal", "9x", "x"], 9)]
    #[case::truncated(&["pkill", "--signal", "4294967305", "x"], libc::SIGKILL)]
    #[case::darwin_value(&["pkill", "--signal", "INFO", "x"], libc::SIGINFO)]
    #[case::replaced_linux_signal(&["pkill", "-RTMIN", "--signal", "TERM", "x"], libc::SIGTERM)]
    #[case::replaced_linux_value(&["pkill", "--signal", "RTMIN", "--signal", "HUP", "x"], libc::SIGHUP)]
    #[case::default(&["pkill", "x"], libc::SIGTERM)]
    fn pkill_reads_its_signal(
        #[case] arguments: &[&str],
        #[case] signal: libc::c_int,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(Mode::Pkill, arguments)?,
            Options {
                signal,
                ..with_pattern()
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::zero(&["pgrep", "--ns", "0", "x"])]
    #[case::not_a_number(&["pgrep", "--ns", "abc", "x"])]
    #[case::zero_after_pid(&["pgrep", "--ns", "5", "--ns", "0", "x"])]
    fn a_namespace_pid_of_0_selects_by_state(
        #[case] arguments: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let states = OsString::from(arguments[arguments.len() - 2]);

        assert_eq!(
            options(Mode::Pgrep, arguments)?,
            Options {
                filters: Filters {
                    states: Some(states),
                    ..Filters::default()
                },
                ..with_pattern()
            }
        );

        Ok(())
    }

    #[test]
    fn pgrep_ignores_a_linux_signal_without_require_handler()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(Mode::Pgrep, &["pgrep", "--signal", "RTMIN", "x"])?,
            with_pattern()
        );

        Ok(())
    }

    #[test]
    fn pgrep_reads_no_signal_option() {
        assert_matches!(
            parse(Mode::Pgrep, &["pgrep", "-9", "x"]),
            Err(PgrepError::Getopt { program, message })
                if program == "pgrep" && message == b"invalid option -- '9'"
        );
    }

    #[test]
    fn only_the_first_signal_option_is_a_signal() {
        assert_matches!(
            parse(Mode::Pkill, &["pkill", "-KILL", "-9", "x"]),
            Err(PgrepError::Getopt { program, message })
                if program == "pkill" && message == b"invalid option -- '9'"
        );
    }

    #[test]
    fn the_signal_option_is_removed_even_as_another_option_s_value() {
        assert_matches!(
            parse(Mode::Pkill, &["pkill", "-p", "-9"]),
            Err(PgrepError::Getopt { program, message })
                if program == "pkill" && message == b"option requires an argument -- 'p'"
        );
    }

    #[test]
    fn the_signal_option_is_removed_after_the_end_of_the_options()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(Mode::Pkill, &["pkill", "--", "-9", "x"])?,
            Options {
                signal: libc::SIGKILL,
                ..with_pattern()
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::queue(Mode::Pkill, &["pkill", "-q", "1", "x"], LinuxOption::Queue)]
    #[case::mrelease(Mode::Pkill, &["pkill", "--mrelease", "x"], LinuxOption::Mrelease)]
    #[case::cgroup(Mode::Pgrep, &["pgrep", "--cgroup", "a"], LinuxOption::Cgroup)]
    #[case::ns(Mode::Pgrep, &["pgrep", "--ns", "1"], LinuxOption::Ns)]
    #[case::nslist(Mode::Pkill, &["pkill", "--nslist", "net", "x"], LinuxOption::Nslist)]
    #[case::first(Mode::Pgrep, &["pgrep", "--ns", "1", "--cgroup", "a"], LinuxOption::Ns)]
    #[case::emt_option(Mode::Pkill, &["pkill", "-emt", "pts/0"], LinuxOption::Mrelease)]
    #[case::signal_first(
        Mode::Pkill,
        &["pkill", "--signal", "PWR", "--cgroup", "a", "x"],
        LinuxOption::Signal(LinuxSignal::Power)
    )]
    #[case::handler_signal(
        Mode::Pgrep,
        &["pgrep", "-H", "--signal", "RTMIN", "x"],
        LinuxOption::Signal(LinuxSignal::RealTime)
    )]
    #[case::power(Mode::Pkill, &["pkill", "-PWR", "x"], LinuxOption::Signal(LinuxSignal::Power))]
    #[case::real_time(Mode::Pkill, &["pkill", "-rtmin", "x"], LinuxOption::Signal(LinuxSignal::RealTime))]
    #[case::long_signal(
        Mode::Pkill,
        &["pkill", "--signal", "SIGSTKFLT", "x"],
        LinuxOption::Signal(LinuxSignal::StackFault)
    )]
    #[case::real_time_number_with_letters(
        Mode::Pkill,
        &["pkill", "--signal", "40abc", "x"],
        LinuxOption::Signal(LinuxSignal::RealTime)
    )]
    #[case::real_time_number_after_truncation(
        Mode::Pkill,
        &["pkill", "--signal", "4294967328", "x"],
        LinuxOption::Signal(LinuxSignal::RealTime)
    )]
    fn a_linux_only_option_is_recorded_for_refusal(
        #[case] mode: Mode,
        #[case] arguments: &[&str],
        #[case] expected: LinuxOption,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(options(mode, arguments)?.unsupported, Some(expected));

        Ok(())
    }

    #[rstest]
    #[case::help(Mode::Pkill, &["pkill", "-q", "1", "-h"], Ok(Command::Help))]
    #[case::help_after_signal(Mode::Pkill, &["pkill", "-RTMAX", "-h"], Ok(Command::Help))]
    #[case::info_option(Mode::Pkill, &["pkill", "-info", "x"], Err(Failure::Usage))]
    #[case::emt_missing_argument(Mode::Pkill, &["pkill", "-emt"], Err(message("option requires an argument -- 't'")))]
    #[case::conversion(Mode::Pgrep, &["pgrep", "--cgroup", "a", "-p", "z"], Err(message("not a number: z")))]
    #[case::empty_list(Mode::Pgrep, &["pgrep", "--cgroup", ","], Err(message("invalid empty list")))]
    #[case::namespace(Mode::Pgrep, &["pgrep", "--nslist", "bogus", "x"], Err(Failure::UnknownNamespace))]
    #[case::two_patterns(Mode::Pkill, &["pkill", "-m", "a", "b"], Err(message("only one pattern can be provided")))]
    fn a_linux_only_option_is_refused_after_procps_ng_s_checks(
        #[case] mode: Mode,
        #[case] arguments: &[&str],
        #[case] expected: Result<Command, Failure>,
    ) {
        assert_eq!(parse(mode, arguments).map_err(Failure::from), expected);
    }

    // procps-ng checks `--quiet` with `-a` before `--quiet` with `-l`, whatever
    // their order on the command line.
    #[rstest]
    #[case::list_name_first(&["pgrep", "--quiet", "-l", "-a", "x"])]
    #[case::list_full_first(&["pgrep", "--quiet", "-a", "-l", "x"])]
    fn quiet_with_both_listings_is_the_list_full_error(#[case] arguments: &[&str]) {
        assert_eq!(
            parse(Mode::Pgrep, arguments).map_err(Failure::from),
            Err(message(
                "Cannot use --quiet and -a,--list-full options together"
            ))
        );
    }

    #[rstest]
    #[case::queue(&["pgrep", "--queue", "1", "x"], false)]
    #[case::mrelease(&["pgrep", "--mrelease", "x"], false)]
    #[case::echo(&["pgrep", "--echo", "x"], true)]
    fn pgrep_accepts_the_long_options_that_only_change_pkill(
        #[case] arguments: &[&str],
        #[case] echo: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(Mode::Pgrep, arguments)?,
            Options {
                output: Output {
                    echo,
                    ..Output::default()
                },
                ..with_pattern()
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::version_first(&["pgrep", "-V", "-p", "abc"], Ok(Command::Version))]
    #[case::help_first(&["pgrep", "-h", "-p", "abc"], Ok(Command::Help))]
    #[case::list_first(&["pgrep", "-p", "abc", "-V"], Err(message("not a number: abc")))]
    #[case::question_mark(&["pgrep", "-?"], Err(Failure::Usage))]
    #[case::version_before_invalid(&["pgrep", "-V", "-z"], Ok(Command::Version))]
    #[case::help_in_cluster(&["pgrep", "-iiz"], Ok(Command::Help))]
    #[case::question_before_invalid(&["pgrep", "-?", "-z"], Err(Failure::Usage))]
    #[case::list_before_missing(&["pgrep", "-p", "abc", "-d"], Err(message("not a number: abc")))]
    #[case::invalid_before_version(&["pgrep", "-z", "-V"], Err(message("invalid option -- 'z'")))]
    fn options_take_effect_in_command_line_order(
        #[case] arguments: &[&str],
        #[case] expected: Result<Command, Failure>,
    ) {
        assert_eq!(
            parse(Mode::Pgrep, arguments).map_err(Failure::from),
            expected
        );
    }

    #[rstest]
    #[case::invalid(&["pgrep", "-z"], "invalid option -- 'z'")]
    #[case::colon(&["pgrep", "-c:", "x"], "invalid option -- ':'")]
    #[case::unrecognized(&["pgrep", "--bogus"], "unrecognized option '--bogus'")]
    #[case::ambiguous(
        &["pgrep", "--pi", "1"],
        "option '--pi' is ambiguous; possibilities: '--pid' '--pidfile'"
    )]
    #[case::unwanted(&["pgrep", "--count=1"], "option '--count' doesn't allow an argument")]
    #[case::missing(&["pgrep", "-d"], "option requires an argument -- 'd'")]
    #[case::other_tool(&["pgrep", "-e", "x"], "invalid option -- 'e'")]
    fn a_getopt_error_has_glibc_s_message(#[case] arguments: &[&str], #[case] expected: &str) {
        assert_matches!(
            parse(Mode::Pgrep, arguments),
            Err(PgrepError::Getopt { program, message })
                if program == "pgrep" && message == expected.as_bytes()
        );
    }

    #[rstest]
    fn getopt_and_clap_reject_the_same_arguments(
        #[values(Mode::Pgrep, Mode::Pkill)] mode: Mode,
        #[values(&b"-"[..], &b"-c"[..], &b"--"[..])] prefix: &[u8],
    ) {
        let disagreements: Vec<Vec<u8>> = (1..=u8::MAX)
            .map(|byte| [prefix, &[byte]].concat())
            .filter(|argument| {
                let arguments = [
                    OsString::from("pgrep"),
                    OsStr::from_bytes(argument).to_owned(),
                ];
                let rejected = command(mode).try_get_matches_from(&arguments).is_err();
                let found = matches!(getopt::scan(mode, &arguments[1..], false), Stop::Error(_));

                rejected != found
            })
            .collect();

        assert_eq!(disagreements, Vec::<Vec<u8>>::new());
    }

    /// The options of a successful parse, or the error.
    fn outcome(parsed: Result<Command, PgrepError>) -> Result<Options, Failure> {
        match parsed {
            Ok(Command::Run(options)) => Ok(*options),
            Ok(other) => Err(Failure::Message(format!("{other:?}"))),
            Err(error) => Err(Failure::from(error)),
        }
    }

    #[rstest]
    #[case::read(&["pgrep", "-F", "daemon.pid"], Some(5), Ok((Some(vec![5]), true)))]
    #[case::standard_input(&["pgrep", "-F", "-"], Some(5), Ok((Some(vec![5]), false)))]
    #[case::before_later_checks(&["pgrep", "-F", "daemon.pid", "a", "b"], None, Err(message("pidfile not valid")))]
    #[case::with_pid(&["pgrep", "-F", "daemon.pid", "-p", "1"], Some(5), Err(message("Cannot use pidfile and pid option together")))]
    #[case::lock_without(&["pgrep", "-L", "x"], Some(5), Err(message("-L without -F makes no sense")))]
    fn the_pid_file_is_read_where_procps_ng_reads_it(
        #[case] arguments: &[&str],
        #[case] pid: Option<i64>,
        #[case] expected: Result<(Option<Vec<i64>>, bool), Failure>,
    ) {
        let inputs = FakeInputs {
            posixly_correct: false,
            pid,
            case_folding: CaseFolding::Ascii,
        };
        let expected = expected.map(|(pids, opened_pid_file)| Options {
            filters: Filters {
                pids,
                ..Filters::default()
            },
            opened_pid_file,
            ..Options::initial(libc::SIGTERM)
        });

        assert_eq!(
            outcome(parse_with(Mode::Pgrep, arguments, &inputs)),
            expected
        );
    }

    // In a Turkish locale, procps-ng's `strcasecmp` does not match `i` with
    // `I`, so `-int` is not a signal and getopt reads it as `-i -n -t`.
    #[rstest]
    #[case::signal_option(&["pkill", "-int", "foo"], Ok(&["pkill", "-i", "-n", "-t", "foo"][..]))]
    #[case::signal_value(&["pkill", "--signal", "int", "foo"], Err(Failure::from(PgrepError::UnknownSignal("int".into()))))]
    #[case::upper_case(&["pkill", "-INT", "foo"], Ok(&["pkill", "--signal", "INT", "foo"][..]))]
    fn a_turkish_locale_reads_lower_case_i_as_procps_ng_reads_it(
        #[case] arguments: &[&str],
        #[case] expected: Result<&[&str], Failure>,
    ) {
        let turkish = FakeInputs {
            case_folding: CaseFolding::TurkishUtf8,
            ..INPUTS
        };

        assert_eq!(
            outcome(parse_with(Mode::Pkill, arguments, &turkish)),
            expected.and_then(|same| outcome(parse(Mode::Pkill, same)))
        );
    }

    #[rstest]
    #[case::permuted(false, Ok(Report::Count))]
    #[case::posixly_correct(true, Err(message("only one pattern can be provided")))]
    fn posixly_correct_ends_the_options_at_the_first_operand(
        #[case] posixly_correct: bool,
        #[case] expected: Result<Report, Failure>,
    ) {
        let inputs = FakeInputs {
            posixly_correct,
            pid: None,
            case_folding: CaseFolding::Ascii,
        };
        let expected = expected.map(|report| Options {
            output: Output {
                report,
                ..Output::default()
            },
            ..with_pattern()
        });

        assert_eq!(
            outcome(parse_with(Mode::Pgrep, &["pgrep", "x", "-c"], &inputs)),
            expected
        );
    }

    #[rstest]
    #[case::short(&["pgrep", "-t", "-p=x"])]
    #[case::long(&["pgrep", "--terminal", "-p=x"])]
    fn an_option_s_value_is_not_split_at_its_equals_sign(
        #[case] arguments: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(Mode::Pgrep, arguments)?,
            Options {
                filters: Filters {
                    terminals: Some(vec![OsString::from("-p=x")]),
                    ..Filters::default()
                },
                ..Options::initial(libc::SIGTERM)
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::alone(&["pgrep", "-d=x", "y"], Report::List(Listing::Ids))]
    #[case::clustered(&["pgrep", "-cd=x", "y"], Report::Count)]
    fn an_attached_value_keeps_its_equals_sign(
        #[case] arguments: &[&str],
        #[case] report: Report,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            options(Mode::Pgrep, arguments)?,
            Options {
                output: Output {
                    report,
                    delimiter: OsString::from("=x"),
                    ..Output::default()
                },
                pattern: Some(OsString::from("y")),
                ..Options::initial(libc::SIGTERM)
            }
        );

        Ok(())
    }

    #[rstest]
    #[case::number("5", Some(5))]
    #[case::zero("0x", None)]
    #[case::wraps_to_zero("4294967296", None)]
    #[case::wraps_negative("2147483648", Some(i32::MIN))]
    fn older_is_an_int_as_atoi_reads_it(#[case] value: &str, #[case] older: Option<i32>) {
        let expected = Options {
            filters: Filters {
                older,
                ..Filters::default()
            },
            ..Options::initial(libc::SIGTERM)
        };

        assert_eq!(
            outcome(parse(Mode::Pgrep, &["pgrep", "-O", value])),
            Ok(expected)
        );
    }

    #[rstest]
    #[case::one("4", Ok(vec!["4"]))]
    #[case::empty_elements(",4,,5,", Ok(vec!["4", "5"]))]
    #[case::only_a_comma(",", Err(message("invalid empty list")))]
    #[case::only_commas(",,", Err(message("invalid empty list")))]
    #[case::empty("", Err(Failure::Usage))]
    fn a_list_is_split_as_split_list_splits_it(
        #[case] value: &str,
        #[case] expected: Result<Vec<&str>, Failure>,
    ) {
        let outcome = list(value.as_ref(), |element| Ok(element.to_owned())).map_err(Failure::from);
        let expected = expected.map(|elements| elements.into_iter().map(OsString::from).collect());

        assert_eq!(outcome, expected);
    }

    #[rstest]
    #[case::signed("-12", Some(-12))]
    #[case::sign_only("+", Some(0))]
    #[case::letters("1x", None)]
    #[case::largest("9223372036854775799", Some(9_223_372_036_854_775_799))]
    #[case::below_maximum("9223372036854775800", None)]
    fn a_strict_number_has_strict_atol_s_limits(
        #[case] value: &str,
        #[case] expected: Option<i64>,
    ) {
        assert_eq!(strict_number(value.as_ref()), expected);
    }
}
