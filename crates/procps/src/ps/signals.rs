// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 1998-2003 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/display.c.

use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use darwin_proc::SignalResponse;

use crate::program::PROCPS_NG_VERSION;

/// The signals that procps-ng's `ps` catches and Darwin has. procps-ng
/// catches every signal from 1 to 31 except `SIGINT`, `SIGQUIT`, `SIGKILL`,
/// `SIGSTOP`, `SIGCONT`, `SIGTSTP`, `SIGTTOU`, `SIGPROF`, `SIGWINCH`, `SIGURG`
/// and `SIGCHLD`. Linux has no `SIGEMT` or `SIGINFO`, so Darwin's keep their
/// default actions.
const CAUGHT: [libc::c_int; 18] = [
    libc::SIGHUP,
    libc::SIGILL,
    libc::SIGTRAP,
    libc::SIGABRT,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGUSR1,
    libc::SIGSEGV,
    libc::SIGUSR2,
    libc::SIGPIPE,
    libc::SIGALRM,
    libc::SIGTERM,
    libc::SIGTTIN,
    libc::SIGXCPU,
    libc::SIGXFSZ,
    libc::SIGVTALRM,
    libc::SIGIO,
    libc::SIGSYS,
];

/// The source position that procps-ng's message about an unexpected signal
/// reports.
const POSITION: &str = "src/ps/display.c:75";

/// What `ps` does for each signal that procps-ng's `signal_handler` catches.
/// `SIGPIPE` makes `ps` exit with status 0, and `SIGHUP` with status 1. For
/// another signal, `ps` prints which signal it caught: `SIGUSR1` and `SIGUSR2`
/// then make it exit with status 1, and the others, apart from the silent
/// `SIGTERM`, make it also report a bug before it takes the signal's default
/// action. `argument_zero` is `argv[0]`, and `program` is the name that `ps`
/// was run as, without its directory.
pub(crate) fn responses(
    argument_zero: &OsStr,
    program: &OsStr,
) -> Vec<(libc::c_int, SignalResponse)> {
    CAUGHT
        .iter()
        .map(|&signal| {
            let caught = [
                format!(
                    "Signal {signal} ({}) caught by ",
                    crate::signal::name(signal).unwrap_or("-")
                )
                .as_bytes(),
                program.as_bytes(),
                format!(" ({PROCPS_NG_VERSION}).\n").as_bytes(),
            ]
            .concat();
            let bug = [
                argument_zero.as_bytes(),
                format!(":{POSITION}: please report this bug\n").as_bytes(),
            ]
            .concat();

            let response = match signal {
                libc::SIGPIPE => SignalResponse::Exit {
                    message: Vec::new(),
                    status: 0,
                },
                libc::SIGHUP => SignalResponse::Exit {
                    message: Vec::new(),
                    status: 1,
                },
                libc::SIGUSR1 | libc::SIGUSR2 => SignalResponse::Exit {
                    message: caught,
                    status: 1,
                },
                libc::SIGTERM => SignalResponse::Default {
                    message: Vec::new(),
                },
                _ => SignalResponse::Default {
                    message: [caught, bug].concat(),
                },
            };

            (signal, response)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use darwin_proc::SignalResponse;
    use pretty_assertions::assert_eq;

    use super::responses;

    #[test]
    fn ps_responds_to_signals_as_procps_ng_does() {
        let caught =
            |number: i32, name: &str| format!("Signal {number} ({name}) caught by ps (4.0.7).\n");
        let bug = "/bin/ps:src/ps/display.c:75: please report this bug\n";
        let exit = |message: String, status| SignalResponse::Exit {
            message: message.into_bytes(),
            status,
        };
        let default = |message: String| SignalResponse::Default {
            message: message.into_bytes(),
        };
        let report = |number: i32, name: &str| default(caught(number, name) + bug);

        assert_eq!(
            responses("/bin/ps".as_ref(), "ps".as_ref()),
            vec![
                (libc::SIGHUP, exit(String::new(), 1)),
                (libc::SIGILL, report(4, "ILL")),
                (libc::SIGTRAP, report(5, "TRAP")),
                (libc::SIGABRT, report(6, "ABRT")),
                (libc::SIGBUS, report(10, "BUS")),
                (libc::SIGFPE, report(8, "FPE")),
                (libc::SIGUSR1, exit(caught(30, "USR1"), 1)),
                (libc::SIGSEGV, report(11, "SEGV")),
                (libc::SIGUSR2, exit(caught(31, "USR2"), 1)),
                (libc::SIGPIPE, exit(String::new(), 0)),
                (libc::SIGALRM, report(14, "ALRM")),
                (libc::SIGTERM, default(String::new())),
                (libc::SIGTTIN, report(21, "TTIN")),
                (libc::SIGXCPU, report(24, "XCPU")),
                (libc::SIGXFSZ, report(25, "XFSZ")),
                (libc::SIGVTALRM, report(26, "VTALRM")),
                (libc::SIGIO, report(23, "POLL")),
                (libc::SIGSYS, report(12, "SYS")),
            ]
        );
    }
}
