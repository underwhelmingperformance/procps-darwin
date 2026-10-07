// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2023 Roman Žilka <roman.zilka@gmail.com>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 local/signals.c.

use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use darwin_proc::CaseFolding;

use crate::c_number::strtol;

/// Darwin's signals, by procps-ng's names and in procps-ng's order. The table
/// also has Darwin's `EMT` and `INFO`, which Linux lacks, at the positions that
/// procps-ng's table gives them.
///
/// Darwin has no `SIGCLD`, and defines `SIGPOLL` only in strict POSIX mode,
/// with the number of `SIGEMT`. Linux scripts use both names, so the table maps
/// them to Darwin's `SIGCHLD` and `SIGIO`, which have the same meaning as on
/// Linux.
///
/// [`lookup`] recognises the names of the signals that Darwin lacks.
const SIGNALS: [(&str, libc::c_int); 34] = [
    ("HUP", libc::SIGHUP),
    ("INT", libc::SIGINT),
    ("QUIT", libc::SIGQUIT),
    ("ILL", libc::SIGILL),
    ("TRAP", libc::SIGTRAP),
    ("ABRT", libc::SIGABRT),
    ("IOT", libc::SIGIOT),
    ("BUS", libc::SIGBUS),
    ("EMT", libc::SIGEMT),
    ("FPE", libc::SIGFPE),
    ("KILL", libc::SIGKILL),
    ("USR1", libc::SIGUSR1),
    ("SEGV", libc::SIGSEGV),
    ("USR2", libc::SIGUSR2),
    ("PIPE", libc::SIGPIPE),
    ("ALRM", libc::SIGALRM),
    ("TERM", libc::SIGTERM),
    ("CHLD", libc::SIGCHLD),
    ("CLD", libc::SIGCHLD),
    ("CONT", libc::SIGCONT),
    ("STOP", libc::SIGSTOP),
    ("TSTP", libc::SIGTSTP),
    ("TTIN", libc::SIGTTIN),
    ("TTOU", libc::SIGTTOU),
    ("URG", libc::SIGURG),
    ("XCPU", libc::SIGXCPU),
    ("XFSZ", libc::SIGXFSZ),
    ("VTALRM", libc::SIGVTALRM),
    ("PROF", libc::SIGPROF),
    ("WINCH", libc::SIGWINCH),
    ("POLL", libc::SIGIO),
    ("IO", libc::SIGIO),
    ("INFO", libc::SIGINFO),
    ("SYS", libc::SIGSYS),
];

/// A Linux signal that Darwin lacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinuxSignal {
    /// `SIGPWR`.
    Power,
    /// `SIGSTKFLT`.
    StackFault,
    /// A real-time signal: `RTMIN`, `RTMAX`, `RTMIN+n`, `RTMAX-n`, or a number
    /// from 32 to 64.
    RealTime,
}

/// What a signal name or number refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lookup {
    /// A signal number, which is used unchanged.
    Signal(libc::c_int),
    /// A signal that procps-ng accepts on Linux but Darwin lacks.
    LinuxOnly(LinuxSignal),
    /// Neither a signal name nor a signal number.
    Unknown,
}

/// The difference between glibc's `SIGRTMAX` and `SIGRTMIN`, which limits
/// the `n` of `RTMIN+n` and `RTMAX-n`.
const REAL_TIME_SIGNALS: i64 = 64 - 34;

/// The numbers of Linux's real-time signals, from the kernel's `SIGRTMIN` to
/// its `SIGRTMAX`. glibc reserves 32 and 33 for its threads, but `kill(2)`
/// sends them. Darwin's signals end at 31, so Darwin has none of these.
const LINUX_REAL_TIME_NUMBERS: std::ops::RangeInclusive<libc::c_int> = 32..=64;

/// Where a signal name appears on the command line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Form {
    /// As `pkill`'s `-<signal>` argument.
    Option,
    /// As the value of `--signal`.
    Value,
}

/// The signals that Darwin has and Linux lacks. Linux has no `EMT` or `INFO`
/// signal, so procps-ng reads `pkill -emt` as `-e -m -t` and `pkill -info` as
/// `-i -n -f -o`, and the tools read them the same way. As the value of
/// `--signal`, the tools accept these names, because Darwin has the signals,
/// whereas procps-ng on Linux reports an unknown signal.
const DARWIN_ONLY: [&str; 2] = ["EMT", "INFO"];

/// What `name` refers to, as procps-ng's `signal_name_to_number` reads it: a
/// decimal number, or a signal name with or without `SIG`, in upper or lower
/// case. Like procps-ng, it reads a number as `strtol` does, and accepts any
/// number from 0 to `INT_MAX`, whether or not Darwin has such a signal. A
/// number of a Linux real-time signal is [`LinuxSignal::RealTime`]. In
/// the [`Form::Option`] form, the names in [`DARWIN_ONLY`] (`EMT` and
/// `INFO`) are unknown. procps-ng compares names with `strcasecmp`, which
/// folds case as the locale does, and `folding` gives that locale's rules.
pub(crate) fn lookup(name: &OsStr, form: Form, folding: CaseFolding) -> Lookup {
    let name = name.as_bytes();

    if let Some(number) = strtol(name) {
        if number.length != name.len() || number.overflowed {
            return Lookup::Unknown;
        }

        return match libc::c_int::try_from(number.value) {
            Ok(number) if number >= 0 => from_number(number),
            _ => Lookup::Unknown,
        };
    }

    let name = if folding.starts_with_ignore_case(name, b"SIG") {
        &name[3..]
    } else {
        name
    };

    if let Some(&(known, number)) = SIGNALS
        .iter()
        .find(|(known, _)| folding.eq_ignore_case(name, known.as_bytes()))
    {
        if form == Form::Option && DARWIN_ONLY.contains(&known) {
            return Lookup::Unknown;
        }

        return Lookup::Signal(number);
    }

    if folding.eq_ignore_case(name, b"PWR") {
        return Lookup::LinuxOnly(LinuxSignal::Power);
    }

    if folding.eq_ignore_case(name, b"STKFLT") {
        return Lookup::LinuxOnly(LinuxSignal::StackFault);
    }

    if is_real_time(name, folding) {
        return Lookup::LinuxOnly(LinuxSignal::RealTime);
    }

    Lookup::Unknown
}

/// The signal with the number `number`: a Linux real-time signal for the
/// numbers 32 to 64, and otherwise the number itself.
pub(crate) fn from_number(number: libc::c_int) -> Lookup {
    if LINUX_REAL_TIME_NUMBERS.contains(&number) {
        return Lookup::LinuxOnly(LinuxSignal::RealTime);
    }

    Lookup::Signal(number)
}

/// Whether `name`, without `SIG`, is a real-time signal that procps-ng
/// accepts on Linux.
fn is_real_time(name: &[u8], folding: CaseFolding) -> bool {
    let sign = match name {
        _ if folding.starts_with_ignore_case(name, b"RTMIN") => b'+',
        _ if folding.starts_with_ignore_case(name, b"RTMAX") => b'-',
        _ => return false,
    };
    let offset = &name[5..];

    let Some((&first, number)) = offset.split_first() else {
        return true;
    };

    if first != sign || !number.first().is_some_and(u8::is_ascii_digit) {
        return false;
    }

    strtol(number).is_some_and(|parsed| {
        parsed.length == number.len() && !parsed.overflowed && parsed.value <= REAL_TIME_SIGNALS
    })
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    use darwin_proc::CaseFolding;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Form, LinuxSignal, Lookup, lookup};

    #[rstest]
    #[case::name("TERM", Lookup::Signal(15))]
    #[case::prefixed("SIGKILL", Lookup::Signal(9))]
    #[case::lower_case("sigusr1", Lookup::Signal(30))]
    #[case::darwin_only("INFO", Lookup::Signal(29))]
    #[case::alias("IOT", Lookup::Signal(6))]
    #[case::number("9", Lookup::Signal(9))]
    #[case::last_darwin_number("31", Lookup::Signal(31))]
    #[case::first_real_time_number("32", Lookup::LinuxOnly(LinuxSignal::RealTime))]
    #[case::last_real_time_number("64", Lookup::LinuxOnly(LinuxSignal::RealTime))]
    #[case::after_the_real_time_numbers("65", Lookup::Signal(65))]
    #[case::large_number("12345", Lookup::Signal(12345))]
    #[case::plus("+3", Lookup::Signal(3))]
    #[case::white_space(" 3", Lookup::Signal(3))]
    #[case::negative("-3", Lookup::Unknown)]
    #[case::negative_zero("-0", Lookup::Signal(0))]
    #[case::vertical_tab("\x0b9", Lookup::Signal(9))]
    #[case::child_alias("CLD", Lookup::Signal(libc::SIGCHLD))]
    #[case::poll_alias("sigpoll", Lookup::Signal(libc::SIGIO))]
    #[case::trailing("9x", Lookup::Unknown)]
    #[case::too_large("2147483648", Lookup::Unknown)]
    #[case::power("PWR", Lookup::LinuxOnly(LinuxSignal::Power))]
    #[case::stack_fault("sigstkflt", Lookup::LinuxOnly(LinuxSignal::StackFault))]
    #[case::real_time("rtmin", Lookup::LinuxOnly(LinuxSignal::RealTime))]
    #[case::real_time_offset("RTMIN+30", Lookup::LinuxOnly(LinuxSignal::RealTime))]
    #[case::real_time_from_end("SIGRTMAX-2", Lookup::LinuxOnly(LinuxSignal::RealTime))]
    #[case::real_time_out_of_range("RTMIN+31", Lookup::Unknown)]
    #[case::real_time_not_a_number("RTMIN+x", Lookup::Unknown)]
    #[case::real_time_wrong_sign("RTMAX+1", Lookup::Unknown)]
    #[case::empty("", Lookup::Unknown)]
    fn a_signal_is_read_as_procps_ng_reads_it(#[case] name: &str, #[case] expected: Lookup) {
        assert_eq!(
            lookup(OsStr::new(name), Form::Value, CaseFolding::Ascii),
            expected
        );
    }

    // glibc 2.44's `strcasecmp` gives these results in `tr_TR.UTF-8` and
    // `tr_TR.ISO-8859-9`.
    #[rstest]
    #[case::lower_case_i_in_utf8(CaseFolding::TurkishUtf8, b"int", Lookup::Unknown)]
    #[case::upper_case_i_in_utf8(CaseFolding::TurkishUtf8, b"SIGINT", Lookup::Signal(libc::SIGINT))]
    #[case::prefix_in_utf8(CaseFolding::TurkishUtf8, b"sigterm", Lookup::Unknown)]
    #[case::other_letters_in_utf8(CaseFolding::TurkishUtf8, b"term", Lookup::Signal(libc::SIGTERM))]
    #[case::mixed_case_in_utf8(CaseFolding::TurkishUtf8, b"KiLL", Lookup::Unknown)]
    #[case::dotless_i_in_utf8(CaseFolding::TurkishUtf8, b"\xfdnt", Lookup::Unknown)]
    #[case::real_time_in_utf8(CaseFolding::TurkishUtf8, b"rtmin", Lookup::Unknown)]
    #[case::lower_case_i_in_latin5(CaseFolding::TurkishLatin5, b"kill", Lookup::Unknown)]
    #[case::dotless_i_in_latin5(
        CaseFolding::TurkishLatin5,
        b"k\xfdll",
        Lookup::Signal(libc::SIGKILL)
    )]
    #[case::dotless_prefix_in_latin5(
        CaseFolding::TurkishLatin5,
        b"s\xfdgterm",
        Lookup::Signal(libc::SIGTERM)
    )]
    #[case::dotted_capital_in_latin5(CaseFolding::TurkishLatin5, b"\xddnt", Lookup::Unknown)]
    #[case::dotless_i_in_ascii(CaseFolding::Ascii, b"\xfdnt", Lookup::Unknown)]
    fn a_turkish_locale_folds_i_as_glibc_folds_it(
        #[case] folding: CaseFolding,
        #[case] name: &[u8],
        #[case] expected: Lookup,
    ) {
        assert_eq!(
            lookup(OsStr::from_bytes(name), Form::Value, folding),
            expected
        );
    }

    #[rstest]
    #[case::emt_value("emt", Form::Value, Lookup::Signal(libc::SIGEMT))]
    #[case::emt_option("emt", Form::Option, Lookup::Unknown)]
    #[case::info_option("SIGINFO", Form::Option, Lookup::Unknown)]
    #[case::shared_option("TERM", Form::Option, Lookup::Signal(libc::SIGTERM))]
    fn darwin_s_own_signals_are_only_values(
        #[case] name: &str,
        #[case] form: Form,
        #[case] expected: Lookup,
    ) {
        assert_eq!(lookup(OsStr::new(name), form, CaseFolding::Ascii), expected);
    }
}
