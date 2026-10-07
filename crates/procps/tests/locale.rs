// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs `pgrep` in locales that the reference harness cannot use. Its container
//! has only the C, POSIX and C.UTF-8 locales, so these tests cover locales such
//! as en_US.UTF-8 and ISO 8859-1.

use std::{
    ffi::CStr,
    io::{self, BufRead, BufReader, Write},
    process::{Child, Command, ExitStatus, Stdio},
};

use pretty_assertions::assert_eq;
use rstest::rstest;

/// The line that a child writes once it runs.
const READY: &str = "procps-locale ready";

/// The variable that makes [`child`] run as a child. Without the variable, the
/// ignored test returns at once.
const CHILD: &str = "PROCPS_DARWIN_LOCALE_CHILD";

/// Why a test could not run.
#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("cannot start a process or read its output")]
    Io(#[from] io::Error),
    #[error("the child exited before it was ready, with {status}, after writing {written:?}")]
    NotReady { status: ExitStatus, written: String },
    #[error(
        "the test needs darwin_proc::set_locale to give {name:?} the codeset {codeset:?}, but it \
         gave {found:?}"
    )]
    MissingLocale {
        name: String,
        codeset: Option<&'static str>,
        found: Option<String>,
    },
}

/// Checks that `darwin_proc::set_locale` gives each locale in `locales` its
/// codeset, or fails for a locale whose codeset is `None`. pgrep sets its
/// locale with the same function, so a locale that Darwin lacks fails the test
/// before pgrep runs.
fn require(locales: &[(&CStr, Option<&'static str>)]) -> Result<(), TestError> {
    for &(name, codeset) in locales {
        let found = darwin_proc::set_locale(name).map(|locale| locale.codeset().to_owned());

        if found.as_deref() != codeset {
            return Err(TestError::MissingLocale {
                name: name.to_string_lossy().into_owned(),
                codeset,
                found,
            });
        }
    }

    Ok(())
}

/// A child process that has written [`READY`]. The child is killed and reaped
/// when the value is dropped.
struct Running(Child);

impl Running {
    /// Starts `command` with pipes for its standard input and output, and
    /// returns once it has written [`READY`].
    fn start(command: &mut Command) -> Result<Self, TestError> {
        let mut child = Self(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?,
        );
        let stdout = child
            .0
            .stdout
            .take()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        let mut stdout = BufReader::new(stdout);
        let mut written = String::new();

        loop {
            let mut line = String::new();

            if stdout.read_line(&mut line)? == 0 {
                let status = child.0.wait()?;
                return Err(TestError::NotReady { status, written });
            }

            written.push_str(&line);

            if line.trim_end() == READY {
                return Ok(child);
            }
        }
    }

    /// A shell whose last argument ends with `é`.
    fn shell(marker: &str) -> Result<Self, TestError> {
        Self::start(Command::new("/bin/sh").args([
            "-c",
            &format!("echo {READY}; read line"),
            &format!("{marker}é"),
        ]))
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs as a child of [`a_name_keeps_the_locale_s_printable_bytes`]: writes
/// [`READY`], then waits for its standard input to close.
#[test]
#[ignore = "a_name_keeps_the_locale_s_printable_bytes runs it as a child"]
fn child() -> io::Result<()> {
    if std::env::var_os(CHILD).is_none() {
        return Ok(());
    }

    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{READY}")?;
    stdout.flush()?;

    io::copy(&mut io::stdin().lock(), &mut io::sink())?;

    Ok(())
}

/// Runs `pgrep -f '<marker><ending>$'` with only the locale variables `locale`
/// in its environment, and returns its exit status and standard output.
fn pgrep_in(
    locale: &[(&str, &str)],
    marker: &str,
    ending: &str,
) -> io::Result<(Option<i32>, String)> {
    let output = Command::new(env!("CARGO_BIN_EXE_pgrep"))
        .args(["-f", &format!("{marker}{ending}$")])
        .env_clear()
        .envs(locale.iter().copied())
        .output()?;

    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// What [`pgrep_in`] returns when it selects `shell`, or selects nothing.
fn selection(shell: &Running, selected: bool) -> (Option<i32>, String) {
    if selected {
        return (Some(0), format!("{}\n", shell.pid()));
    }

    (Some(1), String::new())
}

// Each case runs the patterns `.` and `é` at the end of the command line. In a
// UTF-8 locale, both match `é`. In ISO 8859-1, the two UTF-8 bytes of `é` are
// two printable characters: `.` matches only one of them, and the pattern `é`
// is the same two characters, so it matches. In the C locale, the command line
// has `??` in place of `é`, and neither pattern matches. When the environment
// sets one category to a locale that does not exist, procps-ng's
// `setlocale(LC_ALL, "")` fails, and procps-ng and the tools stay in the C
// locale. The tools read `C.utf8` as C.UTF-8, and `en_US` as an ISO 8859-1
// locale, as glibc does.
#[rstest]
#[case::utf8(
    "utf8",
    &[("LC_ALL", "en_US.UTF-8")],
    &[(c"en_US.UTF-8", Some("UTF-8"))],
    (true, true)
)]
#[case::linux_spelling(
    "linux",
    &[("LC_ALL", "C.utf8")],
    &[(c"C.utf8", Some("UTF-8"))],
    (true, true)
)]
#[case::no_codeset(
    "bare",
    &[("LANG", "en_US")],
    &[(c"en_US", Some("ISO8859-1"))],
    (false, true)
)]
#[case::unknown_category(
    "unknown",
    &[("LANG", "en_US.UTF-8"), ("LC_TIME", "bogus_XX")],
    &[(c"en_US.UTF-8", Some("UTF-8")), (c"bogus_XX", None)],
    (false, false)
)]
fn the_pattern_is_read_in_the_locale(
    #[case] name: &str,
    #[case] locale: &[(&str, &str)],
    #[case] locales: &[(&CStr, Option<&'static str>)],
    #[case] selected: (bool, bool),
) -> Result<(), TestError> {
    require(locales)?;
    let marker = format!("procps-locale-{}-{name}-", std::process::id());
    let shell = Running::shell(&marker)?;

    assert_eq!(
        (
            pgrep_in(locale, &marker, ".")?,
            pgrep_in(locale, &marker, "é")?
        ),
        (selection(&shell, selected.0), selection(&shell, selected.1))
    );

    Ok(())
}

// In a locale with collation rules, glibc compares the ends of a range by the
// rules, and `[à-ü]` matches `é`. The tools compare code points, which give the
// same result for these characters.
#[test]
fn a_range_outside_ascii_selects_in_a_locale_with_collation_rules() -> Result<(), TestError> {
    require(&[(c"en_US.UTF-8", Some("UTF-8"))])?;
    let marker = format!("procps-locale-{}-range-", std::process::id());
    let shell = Running::shell(&marker)?;

    assert_eq!(
        pgrep_in(&[("LC_ALL", "en_US.UTF-8")], &marker, "[à-ü]")?,
        selection(&shell, true)
    );

    Ok(())
}

// In a single-byte locale, procps-ng keeps the bytes that glibc's `isprint`
// accepts in the locale. In ISO 8859-1, both bytes of a UTF-8 `é` are
// printable, so the name `café` stays as it is. The process is a copy of this
// test binary, because macOS kills a copy of a system binary, such as
// `/bin/sh`, that runs from another directory.
#[test]
fn a_name_keeps_the_locale_s_printable_bytes() -> Result<(), TestError> {
    require(&[(c"en_GB.ISO8859-1", Some("ISO8859-1"))])?;
    let directory = tempfile::tempdir()?;
    let program = directory.path().join("café");
    std::fs::copy(std::env::current_exe()?, &program)?;
    let child = Running::start(
        Command::new(&program)
            .args(["--ignored", "--exact", "--quiet", "child"])
            .env(CHILD, "1"),
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_pgrep"))
        .args(["-P", &std::process::id().to_string(), "-x", "café"])
        .env_clear()
        .env("LC_ALL", "en_GB.ISO8859-1")
        .output()?;

    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned()
        ),
        (Some(0), format!("{}\n", child.pid()))
    );

    Ok(())
}
