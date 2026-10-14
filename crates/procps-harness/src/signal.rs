// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use rustix::process::Signal;

/// Signal numbers differ between Linux and macOS, so golden files record each
/// signal by name.
const NAMES: &[(Signal, &str)] = &[
    (Signal::HUP, "SIGHUP"),
    (Signal::INT, "SIGINT"),
    (Signal::QUIT, "SIGQUIT"),
    (Signal::ILL, "SIGILL"),
    (Signal::TRAP, "SIGTRAP"),
    (Signal::ABORT, "SIGABRT"),
    (Signal::BUS, "SIGBUS"),
    (Signal::FPE, "SIGFPE"),
    (Signal::KILL, "SIGKILL"),
    (Signal::USR1, "SIGUSR1"),
    (Signal::SEGV, "SIGSEGV"),
    (Signal::USR2, "SIGUSR2"),
    (Signal::PIPE, "SIGPIPE"),
    (Signal::ALARM, "SIGALRM"),
    (Signal::TERM, "SIGTERM"),
    (Signal::CHILD, "SIGCHLD"),
    (Signal::CONT, "SIGCONT"),
    (Signal::STOP, "SIGSTOP"),
    (Signal::TSTP, "SIGTSTP"),
    (Signal::TTIN, "SIGTTIN"),
    (Signal::TTOU, "SIGTTOU"),
    (Signal::URG, "SIGURG"),
    (Signal::XCPU, "SIGXCPU"),
    (Signal::XFSZ, "SIGXFSZ"),
    (Signal::VTALARM, "SIGVTALRM"),
    (Signal::PROF, "SIGPROF"),
    (Signal::WINCH, "SIGWINCH"),
    (Signal::IO, "SIGIO"),
    (Signal::SYS, "SIGSYS"),
];

/// The signal called `name`, such as `SIGHUP`, if `signal-hook` can register a
/// handler for it. `signal-hook` refuses the signals in its `FORBIDDEN` list.
pub(crate) fn catchable(name: &str) -> Option<Signal> {
    NAMES
        .iter()
        .find(|(_, known)| *known == name)
        .map(|(signal, _)| *signal)
        .filter(|signal| !signal_hook::consts::FORBIDDEN.contains(&signal.as_raw()))
}

/// The name of the signal with the number `raw`, such as `SIGTERM`, or
/// `signal <raw>` for a signal that only one of the systems has.
pub(crate) fn name(raw: i32) -> String {
    NAMES
        .iter()
        .find(|(signal, _)| signal.as_raw() == raw)
        .map_or_else(|| format!("signal {raw}"), |(_, name)| (*name).to_owned())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use rustix::process::Signal;

    #[rstest]
    #[case::termination(Signal::TERM.as_raw(), "SIGTERM")]
    #[case::alarm(Signal::ALARM.as_raw(), "SIGALRM")]
    #[case::unknown(0, "signal 0")]
    fn a_signal_has_its_posix_name(#[case] raw: i32, #[case] expected: &str) {
        assert_eq!(super::name(raw), expected);
    }

    #[rstest]
    #[case::hangup("SIGHUP", Some(Signal::HUP))]
    #[case::user("SIGUSR1", Some(Signal::USR1))]
    #[case::kill("SIGKILL", None)]
    #[case::segmentation_fault("SIGSEGV", None)]
    #[case::without_prefix("HUP", None)]
    fn a_signal_to_catch_is_found_by_name(#[case] name: &str, #[case] expected: Option<Signal>) {
        assert_eq!(super::catchable(name), expected);
    }
}
