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
    ffi::{OsStr, OsString},
    fmt,
    io::{self, Write},
    os::unix::ffi::OsStrExt,
    process::ExitCode,
};

use crate::{
    output::WriteFailure,
    pgrep::{Mode, usage},
    signal::LinuxSignal,
    strerror::{describe, os_message},
};

/// An option for a Linux concept that Darwin lacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinuxOption {
    /// `--cgroup`.
    Cgroup,
    /// `--ns`.
    Ns,
    /// `--nslist`.
    Nslist,
    /// `pkill`'s `-q` or `--queue`.
    Queue,
    /// `pkill`'s `-m` or `--mrelease`.
    Mrelease,
    /// A signal that Darwin lacks, given as `-<signal>` or with `--signal`.
    Signal(LinuxSignal),
}

impl LinuxOption {
    /// The error message for the option.
    const fn message(self) -> &'static str {
        match self {
            Self::Cgroup => "--cgroup is not supported on macOS",
            Self::Ns => "--ns is not supported on macOS",
            Self::Nslist => "--nslist is not supported on macOS",
            Self::Queue => "-q, --queue is not supported on macOS",
            Self::Mrelease => "-m, --mrelease is not supported on macOS",
            Self::Signal(LinuxSignal::Power) => "SIGPWR is not supported on macOS",
            Self::Signal(LinuxSignal::StackFault) => "SIGSTKFLT is not supported on macOS",
            Self::Signal(LinuxSignal::RealTime) => "real-time signals are not supported on macOS",
        }
    }
}

/// An error that ends `pgrep` or `pkill`.
///
/// Each variant has procps-ng's message and exit status, except that
/// [`PgrepError::Unsupported`], [`PgrepError::Choice`] and
/// [`PgrepError::Source`] have this project's own messages. The errors in
/// reading the pid file, `-F` together with `-p`, and failed writes exit with
/// status 1. [`PgrepError::Choice`] and [`PgrepError::Source`] are fatal
/// errors, which exit with status 3, and every other variant exits with status
/// 2.
///
/// [`PgrepError::Usage`], [`PgrepError::UnknownNamespace`] and a failed write
/// to standard error have a message only for `Display`, because procps-ng
/// prints none for them. The `Display` output replaces bytes that are not
/// UTF-8. [`PgrepError::report`] writes the bytes unchanged, as procps-ng
/// does.
///
/// ```
/// use procps::pgrep::{Mode, Options, PgrepError};
///
/// let error = Options::parse(Mode::Pgrep, ["pgrep"].map(Into::into))
///     .expect_err("a command line without criteria is an error");
///
/// assert_eq!(error.to_string(), "no matching criteria specified");
/// ```
#[derive(Debug, thiserror::Error)]
pub enum PgrepError {
    /// The command line has an error that glibc's `getopt_long` reports.
    Getopt {
        /// The program's `argv[0]`, which glibc puts before the message.
        program: OsString,
        /// glibc's message.
        message: Vec<u8>,
    },
    /// The command line is not valid, and procps-ng prints only the usage
    /// text.
    Usage,
    /// A list of namespaces has an unknown namespace. procps-ng exits without
    /// a message.
    UnknownNamespace,
    /// The value of `--signal` is not a signal.
    UnknownSignal(OsString),
    /// A list contains only commas.
    EmptyList,
    /// A list of numbers has an element that is not a number.
    NotANumber(OsString),
    /// A list of users has an unknown user.
    UnknownUser(OsString),
    /// A list of groups has an unknown group.
    UnknownGroup(OsString),
    /// A list of process groups has an element that is not a number.
    InvalidProcessGroup(OsString),
    /// A list of sessions has an element that is not a number.
    InvalidSession(OsString),
    /// `-L` without `-F`.
    LockWithoutPidfile,
    /// `-F` together with `-p`.
    PidfileAndPid,
    /// The pid file cannot be opened.
    OpenPidfile(#[source] io::Error),
    /// `-L` is given, and no other process has locked the pid file. The
    /// error is the error of the last lock call that failed, or `None` if
    /// both lock calls succeeded.
    UnlockedPidfile(#[source] Option<io::Error>),
    /// The pid file does not start with a pid.
    InvalidPidfile,
    /// `--quiet` together with `-a`.
    QuietAndListFull,
    /// `--quiet` together with `-l`.
    QuietAndListName,
    /// More than one pattern.
    TwoPatterns,
    /// No pattern and no option that selects processes.
    NoCriteria,
    /// The option is for a Linux concept that Darwin lacks.
    Unsupported(LinuxOption),
    /// `regcomp` rejected the pattern.
    Regex(#[from] darwin_proc::RegexError),
    /// `PROCPS_DARWIN_SOURCE` has a value other than `local`, `helper` or the
    /// empty string.
    Choice(#[from] procps_core::ChoiceError),
    /// The source could not take a snapshot of the processes.
    Source(#[from] procps_core::SourceError),
    /// A write to standard output or standard error failed.
    Write(#[from] WriteFailure),
}

/// How procps-ng lays out an error on standard error.
enum Shape {
    /// The message, after `argv[0]`, then the usage text.
    AfterArgumentZero,
    /// The message, after the program name, then the usage text.
    WithUsage,
    /// The message without the program name, then the usage text.
    BareWithUsage,
    /// Only the usage text.
    UsageOnly,
    /// Nothing.
    Nothing,
    /// The message, then a line that suggests `--help`.
    WithHelpHint,
    /// Only the message.
    Alone,
}

impl PgrepError {
    /// The exit status that procps-ng uses for this error.
    ///
    /// ```
    /// use procps::pgrep::PgrepError;
    ///
    /// assert_eq!(PgrepError::PidfileAndPid.status(), 1);
    /// ```
    #[must_use]
    pub const fn status(&self) -> u8 {
        match self {
            Self::PidfileAndPid
            | Self::OpenPidfile(_)
            | Self::UnlockedPidfile(_)
            | Self::InvalidPidfile
            | Self::Write(_) => 1,
            Self::Choice(_) | Self::Source(_) => 3,
            _ => 2,
        }
    }

    const fn shape(&self) -> Shape {
        match self {
            Self::Getopt { .. } => Shape::AfterArgumentZero,
            Self::EmptyList => Shape::WithUsage,
            Self::UnknownSignal(_) => Shape::BareWithUsage,
            Self::Usage => Shape::UsageOnly,
            Self::UnknownNamespace | Self::Write(WriteFailure::Stderr) => Shape::Nothing,
            Self::LockWithoutPidfile
            | Self::PidfileAndPid
            | Self::InvalidPidfile
            | Self::QuietAndListFull
            | Self::QuietAndListName
            | Self::TwoPatterns
            | Self::NoCriteria
            | Self::Unsupported(_) => Shape::WithHelpHint,
            Self::NotANumber(_)
            | Self::UnknownUser(_)
            | Self::UnknownGroup(_)
            | Self::InvalidProcessGroup(_)
            | Self::InvalidSession(_)
            | Self::OpenPidfile(_)
            | Self::UnlockedPidfile(_)
            | Self::Write(WriteFailure::Stdout(_))
            | Self::Regex(_)
            | Self::Choice(_)
            | Self::Source(_) => Shape::Alone,
        }
    }

    /// The message, as bytes.
    fn message(&self) -> Vec<u8> {
        let with_value = |prefix: &[u8], value: &OsString| [prefix, value.as_bytes()].concat();
        let text = |text: &str| text.as_bytes().to_vec();

        match self {
            Self::Getopt { message, .. } => message.clone(),
            Self::Usage => text("invalid usage"),
            Self::UnknownNamespace => text("invalid namespace"),
            Self::UnknownSignal(value) => [b"Unknown signal \"", value.as_bytes(), b"\"."].concat(),
            Self::EmptyList => text("invalid empty list"),
            Self::NotANumber(value) => with_value(b"not a number: ", value),
            Self::UnknownUser(value) => with_value(b"invalid user name: ", value),
            Self::UnknownGroup(value) => with_value(b"invalid group name: ", value),
            Self::InvalidProcessGroup(value) => with_value(b"invalid process group: ", value),
            Self::InvalidSession(value) => with_value(b"invalid session id: ", value),
            Self::LockWithoutPidfile => text("-L without -F makes no sense"),
            Self::PidfileAndPid => text("Cannot use pidfile and pid option together"),
            Self::OpenPidfile(error) => {
                text(&format!("Unable to open pidfile: {}", os_message(error)))
            }
            // When both lock calls succeed, procps-ng prints the description
            // of the `errno` that an earlier library call left, which is 0 in
            // the reference harness in the C locale. The tools always print
            // "Success", glibc's description of 0, whereas Darwin describes 0
            // as "Undefined error: 0".
            Self::UnlockedPidfile(error) => text(&format!(
                "Locking check for pidfile failed: {}",
                error
                    .as_ref()
                    .map_or_else(|| "Success".to_owned(), os_message)
            )),
            Self::InvalidPidfile => text("pidfile not valid"),
            Self::QuietAndListFull => {
                text("Cannot use --quiet and -a,--list-full options together")
            }
            Self::QuietAndListName => {
                text("Cannot use --quiet and -l,--list-name options together")
            }
            Self::TwoPatterns => text("only one pattern can be provided"),
            Self::NoCriteria => text("no matching criteria specified"),
            Self::Unsupported(option) => text(option.message()),
            Self::Write(WriteFailure::Stdout(Some(error))) => {
                text(&format!("write error: {}", os_message(error)))
            }
            Self::Write(failure) => text(&failure.to_string()),
            Self::Regex(error) => text(&format!("regex error: {}", error.message)),
            Self::Choice(error) => text(&describe(error)),
            Self::Source(error) => text(&describe(error)),
        }
    }

    /// Writes the error as procps-ng does, for the tool that `mode` selects,
    /// invoked as `program`, and returns the exit status.
    ///
    /// ```
    /// use procps::pgrep::{Mode, PgrepError};
    ///
    /// let mut stderr = Vec::new();
    /// let status = PgrepError::NoCriteria.report(Mode::Pgrep, "pgrep".as_ref(), &mut stderr);
    ///
    /// assert_eq!(
    ///     String::from_utf8(stderr)?,
    ///     "pgrep: no matching criteria specified\nTry `pgrep --help' for more information.\n"
    /// );
    /// # Ok::<(), std::string::FromUtf8Error>(())
    /// ```
    pub fn report(&self, mode: Mode, program: &OsStr, stderr: &mut impl Write) -> ExitCode {
        // Ignore a failed write: the stream records the failure, and procps-ng
        // reports it only through the exit status.
        let _ = stderr.write_all(&self.render(mode, program));

        ExitCode::from(self.status())
    }

    /// The bytes that procps-ng writes on standard error for this error.
    fn render(&self, mode: Mode, program: &OsStr) -> Vec<u8> {
        let message = self.message();
        let usage = || usage::usage(mode, program);
        let program = program.as_bytes();
        let line = |prefix: &[u8]| [prefix, b": ", &message, b"\n"].concat();

        match self.shape() {
            Shape::AfterArgumentZero => {
                let argument_zero = match self {
                    Self::Getopt { program, .. } => program.as_bytes(),
                    _ => program,
                };

                [line(argument_zero), usage()].concat()
            }
            Shape::WithUsage => [line(program), usage()].concat(),
            Shape::BareWithUsage => [message.clone(), usage()].concat(),
            Shape::UsageOnly => usage(),
            Shape::Nothing => Vec::new(),
            Shape::WithHelpHint => [
                line(program),
                [b"Try `", program, b" --help' for more information.\n"].concat(),
            ]
            .concat(),
            Shape::Alone => line(program),
        }
    }
}

impl fmt::Display for PgrepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&String::from_utf8_lossy(&self.message()))
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::ExitCode};

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::PgrepError;
    use crate::{
        output::WriteFailure,
        pgrep::{Mode, usage::usage},
    };

    fn report(error: &PgrepError) -> (Vec<u8>, ExitCode) {
        let mut stderr = Vec::new();
        let status = error.report(Mode::Pgrep, "pgrep".as_ref(), &mut stderr);

        (stderr, status)
    }

    #[test]
    fn a_getopt_error_starts_with_argument_zero() {
        let error = PgrepError::Getopt {
            program: OsString::from("/tmp/bin/pgrep"),
            message: b"invalid option -- 'z'".to_vec(),
        };
        let expected = [
            b"/tmp/bin/pgrep: invalid option -- 'z'\n".to_vec(),
            usage(Mode::Pgrep, "pgrep".as_ref()),
        ]
        .concat();

        assert_eq!(report(&error), (expected, ExitCode::from(2)));
    }

    #[rstest]
    #[case::raw_bytes(
        PgrepError::NotANumber(OsString::from_vec(b"\xff".to_vec())),
        &b"pgrep: not a number: \xff\n"[..],
        2
    )]
    #[case::open_pidfile(
        PgrepError::OpenPidfile(std::io::Error::from_raw_os_error(libc::ENOENT)),
        &b"pgrep: Unable to open pidfile: No such file or directory\n"[..],
        1
    )]
    #[case::glibc_description(
        PgrepError::OpenPidfile(std::io::Error::from_raw_os_error(libc::ENXIO)),
        &b"pgrep: Unable to open pidfile: No such device or address\n"[..],
        1
    )]
    #[case::namespace(PgrepError::UnknownNamespace, &b""[..], 2)]
    #[case::write(
        PgrepError::Write(WriteFailure::Stdout(Some(std::io::Error::from_raw_os_error(libc::EBADF)))),
        &b"pgrep: write error: Bad file descriptor\n"[..],
        1
    )]
    #[case::earlier_write(
        PgrepError::Write(WriteFailure::Stdout(None)),
        &b"pgrep: write error\n"[..],
        1
    )]
    #[case::stderr(PgrepError::Write(WriteFailure::Stderr), &b""[..], 1)]
    fn an_error_is_reported_as_procps_ng_reports_it(
        #[case] error: PgrepError,
        #[case] expected: &[u8],
        #[case] status: u8,
    ) {
        assert_eq!(report(&error), (expected.to_vec(), ExitCode::from(status)));
    }

    #[test]
    fn a_source_error_has_the_c_library_s_description() {
        let error = PgrepError::Source(procps_core::SourceError::ProcessTable(
            darwin_proc::Error::Os {
                call: darwin_proc::Call::ProcessTable,
                source: std::io::Error::from_raw_os_error(libc::EPERM),
            },
        ));

        assert_eq!(
            report(&error),
            (
                b"pgrep: cannot list processes: sysctl kern.proc: Operation not permitted\n"
                    .to_vec(),
                ExitCode::from(3)
            )
        );
    }
}
