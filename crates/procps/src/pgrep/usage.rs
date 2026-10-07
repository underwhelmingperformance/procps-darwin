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

use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use crate::pgrep::Mode;

/// The options that only `pgrep` lists.
const PGREP_OPTIONS: &str = r" -d, --delimiter <string>  specify output delimiter
 -l, --list-name           list PID and process name
 -a, --list-full           list PID and full command line
     --quiet               suppress all normal output
 -v, --inverse             negates the matching
 -w, --lightweight         list all TID
";

/// The options that only `pkill` lists.
const PKILL_OPTIONS: &str = r" -<sig>                    signal to send (either number or name)
 -H, --require-handler     match only if signal handler is present
 -q, --queue <value>       integer value to be sent with the signal
 -e, --echo                display what is killed
 -m, --mrelease            release process memory immediately after kill
";

/// The options that both tools list.
const COMMON_OPTIONS: &str = r" -c, --count               count of matching processes
 -f, --full                use full command line to match
 -g, --pgroup <PGID,...>   match listed process group IDs
 -G, --group <GID,...>     match real group IDs
 -i, --ignore-case         match case insensitively
 -n, --newest              select most recently started
 -o, --oldest              select least recently started
 -O, --older <seconds>     select where older than seconds
 -p, --pid <PID,...>       match process PIDs
 -P, --parent <PPID,...>   match only child processes of the given parent
 -s, --session <SID,...>   match session IDs
     --signal <sig>        signal to send (either number or name)
 -t, --terminal <tty,...>  match by controlling terminal
 -u, --euid <ID,...>       match by effective IDs
 -U, --uid <ID,...>        match by real IDs
 -x, --exact               match exactly with the command name
 -F, --pidfile <file>      read PIDs from file
 -L, --logpidfile          fail if PID file is not locked
 -r, --runstates <state>   match runstates [D,S,Z,...]
 -A, --ignore-ancestors    exclude our ancestors from results
 -Q, --shell-quote         output the command line in shell-quoted form
 --cgroup <grp,...>        match by cgroup v2 names
 --ns <PID>                match the processes that belong to the same
                           namespace as <pid>
 --nslist <ns,...>         list which namespaces will be considered for
                           the --ns option.
                           Available namespaces: ipc, mnt, net, pid, user, uts
  --env <name=val,...>     match on environment variable

 -h, --help     display this help and exit
 -V, --version  output version information and exit
";

/// The usage text that procps-ng prints for `--help` and after a usage error.
/// `program` is the name that the tool was run as, without its directory.
pub(crate) fn usage(mode: Mode, program: &OsStr) -> Vec<u8> {
    let options = match mode {
        Mode::Pgrep => PGREP_OPTIONS,
        Mode::Pkill => PKILL_OPTIONS,
    };

    [
        b"\nUsage:\n ".as_slice(),
        program.as_bytes(),
        b" [options] <pattern>\n\nOptions:\n",
        options.as_bytes(),
        COMMON_OPTIONS.as_bytes(),
        b"\nFor more details see pgrep(1).\n",
    ]
    .concat()
}
