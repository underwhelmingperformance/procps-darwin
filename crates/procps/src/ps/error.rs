// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2025 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 1998-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/parser.c, src/ps/sortformat.c,
// src/ps/select.c and src/ps/display.c.

use std::{
    ffi::{OsStr, OsString},
    io::Write,
    os::unix::ffi::OsStrExt,
    process::ExitCode,
};

use crate::{
    output::WriteFailure,
    ps::{
        help::{self, Section},
        options::Screen,
    },
    strerror::describe,
};

/// The longest message that procps-ng keeps for an unknown format specifier:
/// its buffer has 80 bytes, and the last is the NUL.
const UNKNOWN_SPECIFIER_MAX: usize = 79;

/// An error in a `ps` command line, with procps-ng's message.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SyntaxError {
    /// An option that must be the only argument has others with it.
    #[error("the option is exclusive: {0}")]
    Exclusive(&'static str),
    /// A pid is not a number.
    #[error("process ID list syntax error")]
    PidSyntax,
    /// A pid is out of range.
    #[error("process ID out of range")]
    PidRange,
    /// A user name is unknown.
    #[error("user name does not exist")]
    UnknownUser,
    /// A user ID is out of range.
    #[error("user ID out of range")]
    UserRange,
    /// A group name is unknown.
    #[error("group name does not exist")]
    UnknownGroup,
    /// A group ID is out of range.
    #[error("group ID out of range")]
    GroupRange,
    /// No terminal has the name.
    #[error("TTY could not be found")]
    TerminalNotFound,
    /// A terminal's path is not a character device.
    #[error("list member was not a TTY")]
    NotATerminal,
    /// A list is empty or has an empty element.
    #[error("improper list")]
    ImproperList,
    /// `-C` has no list.
    #[error("list of command names must follow -C")]
    NoCommands,
    /// `-D` or `--date-format` has no format.
    #[error("date format must follow {0}")]
    NoDateFormat(&'static str),
    /// `-G` or `--Group` has no list.
    #[error("list of real groups must follow {0}")]
    NoRealGroups(&'static str),
    /// `-O` or `O` has no list.
    #[error("format or sort specification must follow {0}")]
    NoFormatOrSort(&'static str),
    /// `-U` or `--User` has no list.
    #[error("list of real users must follow {0}")]
    NoRealUsers(&'static str),
    /// `-g` has no list.
    #[error("list of session leaders OR effective group names must follow -g")]
    NoSessionsOrGroups,
    /// `-g` has a list that is neither sessions nor groups.
    #[error("list of session leaders OR effective group IDs was invalid")]
    InvalidSessionsOrGroups,
    /// `-o`, `o` or `--format` has no list.
    #[error("format specification must follow {0}")]
    NoFormat(&'static str),
    /// `-p`, `p`, `--pid` or `--ppid` has no list.
    #[error("list of process IDs must follow {0}")]
    NoPids(&'static str),
    /// `-q`, `q` or `--quick-pid` has no list.
    #[error("List of process IDs must follow {0}.")]
    NoQuickPids(&'static str),
    /// `-s` has no list.
    #[error("list of session IDs must follow -s")]
    NoSessions,
    /// `-t` has no list.
    #[error("list of terminals (pty, tty...) must follow -t")]
    NoTerminals,
    /// `-u` or `U` has no list.
    #[error("list of users must follow {0}")]
    NoUsers(&'static str),
    /// `-x` without a personality that defines it.
    #[error("must set personality to get -x option")]
    PersonalityForX,
    /// A `-` inside a cluster of UNIX options.
    #[error("embedded '-' among SysV options makes no sense")]
    DashAmongUnixOptions,
    /// An unknown UNIX option.
    #[error("unsupported SysV option")]
    UnknownUnixOption,
    /// BSD's `W`.
    #[error("obsolete W option not supported (you have a /dev/drum?)")]
    ObsoleteW,
    /// More than one of `h`, `--headers` and `--no-headers`.
    #[error("only one heading option may be specified")]
    TwoHeadingOptions,
    /// `k` has no list.
    #[error("long sort specification must follow {0}")]
    NoSort(&'static str),
    /// A `-` inside a cluster of BSD options.
    #[error("embedded '-' among BSD options makes no sense")]
    DashAmongBsdOptions,
    /// An unknown BSD option.
    #[error("unsupported option (BSD syntax)")]
    UnknownBsdOption,
    /// An unknown long option.
    #[error("unknown gnu long option")]
    UnknownLongOption,
    /// `--cols`, `--columns` or `--width` without a valid number.
    #[error("number of columns must follow --cols, --width, or --columns")]
    NoColumns,
    /// A long option that takes no argument has one.
    #[error("option {0} does not take an argument")]
    UnwantedArgument(&'static str),
    /// `--delimiter` has no character.
    #[error("delimiter character must follow --delimiter")]
    NoDelimiter,
    /// `--delimiter` has more than one character.
    #[error("delimiter must be a single character")]
    LongDelimiter,
    /// `--group` has no list.
    #[error("list of effective groups must follow --group")]
    NoEffectiveGroups,
    /// `--rows` or `--lines` without a valid number.
    #[error("number of rows must follow --rows or --lines")]
    NoRows,
    /// `--sid` has no list.
    #[error("some sid thing(s) must follow --sid")]
    NoSids,
    /// `--tty` has no list.
    #[error("list of ttys must follow --tty")]
    NoTtys,
    /// `--user` has no list.
    #[error("list of effective users must follow --user")]
    NoEffectiveUsers,
    /// BSD syntax for an option that the second parse reads as BSD's.
    #[error("way bad")]
    WayBad,
    /// An argument that is not an option.
    #[error("garbage option")]
    Garbage,
    /// A thread display together with a process tree.
    #[error("thread display conflicts with forest display")]
    ThreadsWithForest,
    /// `H` together with `m` or `-m`.
    #[error("thread flags conflict; can't use H with m or -m")]
    HWithM,
    /// `m` together with `-m`.
    #[error("thread flags conflict; can't use both m and -m")]
    BothM,
    /// `-L` together with `-T`.
    #[error("thread flags conflict; can't use both -L and -T")]
    LWithT,
    /// A `%` without a descriptor in an AIX format.
    #[error("missing AIX field descriptor")]
    MissingAixDescriptor,
    /// An AIX format that does not start with `%` or a space.
    #[error("improper AIX field descriptor")]
    ImproperAixDescriptor,
    /// An unknown AIX descriptor.
    #[error("unknown AIX field descriptor")]
    UnknownAixDescriptor,
    /// An unknown format specifier. procps-ng keeps the message of the first
    /// unknown specifier in a static buffer and reports it for every later
    /// one, so this is the first unknown specifier.
    #[error("unknown user-defined format specifier \"{}\"", .0.to_string_lossy())]
    UnknownSpecifier(OsString),
    /// A format list without specifiers.
    #[error("empty format list")]
    EmptyFormatList,
    /// A format list with an empty element.
    #[error("improper format list")]
    ImproperFormatList,
    /// A width that is not a positive decimal number.
    #[error("column widths must be unsigned decimal numbers")]
    BadWidth,
    /// A width for a macro.
    #[error("can not set width for a macro (multi-column) format specifier")]
    MacroWidth,
    /// A sort list with an empty element.
    #[error("improper sort list")]
    ImproperSortList,
    /// A sort list without keys.
    #[error("empty sort list")]
    EmptySortList,
    /// An unknown sort key.
    #[error("unknown sort specifier")]
    UnknownSortKey,
    /// A list of BSD's sort codes with an unknown or repeated code.
    #[error("bad sorting code")]
    BadSortCode,
    /// BSD's `P` sort code together with a process tree.
    #[error("PPID sort and forest output conflict")]
    ParentSortWithForest,
    /// `-O` after another format option.
    #[error("option -O can not follow other format options")]
    LateCapitalO,
    /// More than one sort option.
    #[error("multiple sort options")]
    MultipleSorts,
    /// BSD's `O` after a format option, with a list that is not sort codes.
    #[error("option O is neither first format nor sort order")]
    LateO,
    /// `PS_FORMAT` together with an option that needs an `LWP` or `SPID`
    /// column.
    #[error("tell <procps@freelists.org> what you want (-L/-T, -m/m/H, and $PS_FORMAT)")]
    ThreadsWithPsFormat,
    /// More than one predefined format, or a predefined format together with
    /// a format list.
    #[error("conflicting format options")]
    ConflictingFormats,
    /// A format modifier together with a format list.
    #[error("can not use output modifiers with user-defined output")]
    ModifierWithFormatList,
    /// A format list together with options that need an `LWP` or `SPID`
    /// column.
    #[error("-L/-T with H/m/-m and -o/-O/o/O is nonsense")]
    ThreadsWithFormatList,
    /// `-j` with a predefined format, and no `PGID` column for the `SID`
    /// column to follow.
    #[error("lost my PGID")]
    LostPgid,
    /// `-c` with a predefined format, and no `CLS` column for the `PRI`
    /// column to follow.
    #[error("lost my CLS")]
    LostCls,
    /// `-T` with `H`, `m` or `-m`, and a format without a `PID` column.
    #[error("-T with H/-m/m but no PID for SPID to follow")]
    SpidWithoutPid,
    /// `-L` with `H`, `m` or `-m`, and a format without a column for the
    /// `LWP` column to follow.
    #[error("-L with H/-m/m but no PID/PGID/SID/SESS for NLWP to follow")]
    LwpWithoutPid,
    /// `-y` without `-l`.
    #[error("modifier -y without format -l makes no sense")]
    YWithoutL,
    /// Selection options that cannot be combined.
    #[error("process selection options conflict")]
    SelectionConflict,
}

impl SyntaxError {
    /// The message, as bytes. The message of an unknown specifier contains
    /// the specifier's bytes unchanged, cut to the size of procps-ng's
    /// buffer.
    fn message(&self) -> Vec<u8> {
        let Self::UnknownSpecifier(name) = self else {
            return self.to_string().into_bytes();
        };

        let mut message = [
            b"unknown user-defined format specifier \"",
            name.as_bytes(),
            b"\"",
        ]
        .concat();

        message.truncate(UNKNOWN_SPECIFIER_MAX);
        message
    }
}

/// A warning that procps-ng prints while it reads the command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Warning {
    /// The screen is narrower than 9 columns or shorter than 2 rows.
    ScreenSize(Screen),
    /// `PS_FORMAT` is not a valid format list, so the default format is
    /// used.
    PsFormatIgnored(SyntaxError),
}

impl Warning {
    /// The line that procps-ng prints on standard error.
    pub(crate) fn render(&self) -> Vec<u8> {
        match self {
            Self::ScreenSize(screen) => format!(
                "your {}x{} screen size is bogus. expect trouble\n",
                screen.columns, screen.rows
            )
            .into_bytes(),
            Self::PsFormatIgnored(error) => [
                b"warning: $PS_FORMAT ignored. (".as_slice(),
                &error.message(),
                b")\n",
            ]
            .concat(),
        }
    }
}

/// An option that `-q`, `q` or `--quick-pid` cannot be used with.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum QuickPidConflict {
    /// A second quick selection.
    #[error("q/-q/--quick-pid can only be used once.")]
    Twice,
    /// Another selection option.
    #[error("q/-q/--quick-pid cannot be combined with other selection options.")]
    Selection,
    /// A process tree.
    #[error("q/-q/--quick-pid cannot be used together with forest type listings.")]
    Forest,
    /// A sort option.
    #[error("q/-q,--quick-pid cannot be used together with sort options.")]
    Sort,
    /// `-N` or `--deselect`.
    #[error("q/-q/--quick-pid cannot be used together with negation switches.")]
    Negation,
}

/// A failed check that procps-ng reports as an internal error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InternalError {
    /// `-j` with a format that has neither a `PPID` nor a `PID` column.
    #[error("internal error: no PID or PPID for -j option")]
    NoPidForJ,
    /// `-c` with a format that has no `PRI` column.
    #[error("internal error: no PRI for -c option")]
    NoPriForC,
}

impl InternalError {
    /// The source position that procps-ng's message reports.
    const fn position(self) -> &'static str {
        match self {
            Self::NoPidForJ => "src/ps/sortformat.c:902",
            Self::NoPriForC => "src/ps/sortformat.c:917",
        }
    }
}

/// A part of `ps` that this project does not implement yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum Unimplemented {
    /// A process tree: `-H`, `f` and `--forest` (task 5.5).
    #[error("the process tree is not implemented yet")]
    Forest,
    /// An option that changes how columns show their values (task 5.6).
    #[error("the option {0} is not implemented yet")]
    Option(&'static str),
}

/// The exit status of every error, as in procps-ng.
const STATUS: u8 = 1;

/// An error that ends `ps`.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PsError {
    /// The command line has an error. procps-ng prints the message and the
    /// summary of its usage text.
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    /// `-q` is combined with an option that it cannot be used with.
    #[error(transparent)]
    QuickPid(#[from] QuickPidConflict),
    /// A check that procps-ng reports as an internal error failed.
    /// procps-ng prints the message as glibc's `error_at_line` does, after
    /// `argv[0]` and its source position.
    #[error(transparent)]
    Internal(#[from] InternalError),
    /// The command line needs a part of `ps` that this project does not
    /// implement yet.
    #[error(transparent)]
    Unimplemented(Unimplemented),
    /// `PROCPS_DARWIN_SOURCE` is set to a value that is not a source.
    #[error(transparent)]
    Choice(#[from] procps_core::ChoiceError),
    /// The source could not take a snapshot of the processes.
    #[error("cannot read the processes")]
    Source {
        /// Whether the selection is a list of pids that procps-ng reads
        /// directly, as for `-q` or a `-p` on its own. The message then
        /// contains `select`, and otherwise it contains `reap`.
        quick: bool,
        /// Whether `ps` sorts the processes.
        sorted: bool,
        /// Why the source could not take the snapshot.
        #[source]
        error: procps_core::SourceError,
    },
    /// `ps` sorts the processes, and the snapshot has none.
    #[error("the process table is empty")]
    EmptyProcessTable,
    /// A write to standard output or standard error failed.
    #[error(transparent)]
    Write(#[from] WriteFailure),
}

impl PsError {
    /// The bytes that procps-ng writes on standard error for this error.
    /// `argument_zero` is `argv[0]`, and `program` is the name that `ps` was
    /// run as, without its directory.
    fn render(&self, argument_zero: &OsStr, program: &OsStr) -> Vec<u8> {
        match self {
            Self::Syntax(error) => [
                b"error: ".as_slice(),
                &error.message(),
                b"\n",
                &help::usage(program, Section::Summary),
            ]
            .concat(),
            Self::Internal(error) => [
                argument_zero.as_bytes(),
                b":",
                error.position().as_bytes(),
                b": ",
                error.to_string().as_bytes(),
                b"\n",
            ]
            .concat(),
            Self::Unimplemented(_) => [
                program.as_bytes(),
                b": ",
                self.to_string().as_bytes(),
                b"\n",
            ]
            .concat(),
            Self::Choice(error) => {
                [program.as_bytes(), b": ", describe(error).as_bytes(), b"\n"].concat()
            }
            // procps-ng's `fancy_spew`, which sorts, prints its message
            // without the error's description.
            Self::Source { sorted: true, .. } | Self::EmptyProcessTable => {
                b"fatal library error, reap\n".to_vec()
            }
            Self::Source { quick, error, .. } => {
                let read: &[u8] = if *quick { b"select" } else { b"reap" };

                [
                    program.as_bytes(),
                    b": Fatal library error, unable to ",
                    read,
                    b" pids: ",
                    describe(error).as_bytes(),
                    b"\n",
                ]
                .concat()
            }
            Self::Write(WriteFailure::Stdout(Some(error))) => [
                program.as_bytes(),
                b": write error: ",
                crate::strerror::strerror(error.raw_os_error().unwrap_or_default()).as_bytes(),
                b"\n",
            ]
            .concat(),
            Self::Write(WriteFailure::Stdout(None)) => {
                [program.as_bytes(), b": write error\n"].concat()
            }
            Self::Write(WriteFailure::Stderr) => Vec::new(),
            Self::QuickPid(error) => format!("{error}\n").into_bytes(),
        }
    }

    /// Writes the error as procps-ng does, and returns the exit status.
    /// `argument_zero` is `argv[0]`, and `program` is the name that `ps` was
    /// run as, without its directory.
    pub(crate) fn report(
        &self,
        argument_zero: &OsStr,
        program: &OsStr,
        stderr: &mut impl Write,
    ) -> ExitCode {
        // Ignore a failed write: the stream records the failure, and procps-ng
        // reports it only through the exit status.
        let _ = stderr.write_all(&self.render(argument_zero, program));

        ExitCode::from(STATUS)
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    use pretty_assertions::assert_eq;
    use procps_core::SourceError;
    use rstest::rstest;

    use super::{InternalError, PsError, QuickPidConflict, SyntaxError, Unimplemented};
    use crate::{
        output::WriteFailure,
        ps::help::{Section, usage},
    };

    fn rendered(error: &PsError) -> Vec<u8> {
        error.render("/tmp/ps".as_ref(), "ps".as_ref())
    }

    #[test]
    fn a_syntax_error_has_the_summary_of_the_usage_text() {
        let expected = [
            b"error: list of process IDs must follow p\n".as_slice(),
            &usage("ps".as_ref(), Section::Summary),
        ]
        .concat();

        assert_eq!(
            rendered(&PsError::Syntax(SyntaxError::NoPids("p"))),
            expected
        );
    }

    #[rstest]
    #[case::short("x", &b"unknown user-defined format specifier \"x\""[..])]
    #[case::raw_bytes("\u{fffd}", &b"unknown user-defined format specifier \"\xff\""[..])]
    #[case::long(
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
        &b"unknown user-defined format specifier \"abcdefghijklmnopqrstuvwxyzabcdefghijklmn"[..]
    )]
    fn an_unknown_specifier_is_cut_to_procps_ng_s_buffer(
        #[case] name: &str,
        #[case] expected: &[u8],
    ) {
        let name = if name == "\u{fffd}" {
            OsString::from_vec(vec![0xff])
        } else {
            OsString::from(name)
        };

        assert_eq!(SyntaxError::UnknownSpecifier(name).message(), expected);
    }

    #[rstest]
    #[case::internal(
        PsError::Internal(InternalError::NoPriForC),
        &b"/tmp/ps:src/ps/sortformat.c:917: internal error: no PRI for -c option\n"[..]
    )]
    #[case::quick_pid(
        PsError::QuickPid(QuickPidConflict::Sort),
        &b"q/-q,--quick-pid cannot be used together with sort options.\n"[..]
    )]
    #[case::write(
        PsError::Write(WriteFailure::Stdout(Some(std::io::Error::from_raw_os_error(libc::EBADF)))),
        &b"ps: write error: Bad file descriptor\n"[..]
    )]
    #[case::earlier_write(PsError::Write(WriteFailure::Stdout(None)), &b"ps: write error\n"[..])]
    #[case::stderr(PsError::Write(WriteFailure::Stderr), &b""[..])]
    #[case::unimplemented(
        PsError::Unimplemented(Unimplemented::Option("S")),
        &b"ps: the option S is not implemented yet\n"[..]
    )]
    #[case::source(
        PsError::Source { quick: false, sorted: false, error: SourceError::TimedOut },
        &b"ps: Fatal library error, unable to reap pids: the snapshot reached its time limit\n"[..]
    )]
    #[case::quick_source(
        PsError::Source { quick: true, sorted: false, error: SourceError::TimedOut },
        &b"ps: Fatal library error, unable to select pids: the snapshot reached its time limit\n"[..]
    )]
    #[case::sorted_source(
        PsError::Source { quick: false, sorted: true, error: SourceError::TimedOut },
        &b"fatal library error, reap\n"[..]
    )]
    #[case::empty(PsError::EmptyProcessTable, &b"fatal library error, reap\n"[..])]
    fn an_error_is_written_as_procps_ng_writes_it(#[case] error: PsError, #[case] expected: &[u8]) {
        assert_eq!(rendered(&error), expected);
    }
}
