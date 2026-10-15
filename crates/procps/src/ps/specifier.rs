// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2015-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2011 Lukas Nykryn <lnykryn@redhat.com>
// SPDX-FileCopyrightText: 1999-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/output.c.

/// The number of digits in Darwin's largest pid, 99999. procps-ng makes each
/// column for a pid as wide as the digits of `pid_max - 1` on Linux, and
/// `docs/mappings.md` explains why the tools use Darwin's `PID_MAX`.
const PID_LENGTH: usize = 5;

/// Which rows of `ps`'s output show a column, as procps-ng's `CF_PRINT_*`
/// flags decide when `ps` lists threads after their process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Print {
    /// Only the rows of threads (`CF_PRINT_THREAD_ONLY`).
    ThreadOnly,
    /// Only the rows of processes (`CF_PRINT_PROCESS_ONLY`).
    ProcessOnly,
    /// Every row (`CF_PRINT_EVERY_TIME`).
    EveryTime,
    /// Every row, because procps-ng does not know which rows need the column
    /// (`CF_PRINT_AS_NEEDED`).
    AsNeeded,
}

/// How procps-ng aligns a column's value (`CF_JUST_MASK`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Justification {
    /// On the left for a name and on the right for a number (`CF_USER`).
    User,
    /// On the left (`CF_LEFT`).
    Left,
    /// On the right (`CF_RIGHT`).
    Right,
    /// On the left, with as much width as the line has left
    /// (`CF_UNLIMITED`).
    Unlimited,
    /// On the left for a name and on the right for a number (`CF_WCHAN`).
    Wchan,
    /// On the right, in 9 columns, or 16 on a wide enough screen
    /// (`CF_SIGNAL`).
    Signal,
}

/// A format specifier, with its default header and width.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Specifier {
    /// The name in a format list, such as `pid`.
    pub(crate) name: &'static str,
    /// The header in the C locale.
    pub(crate) header: &'static str,
    /// The width, unless the column shows a pid.
    width: usize,
    /// Whether the column shows a pid and is as wide as the largest pid
    /// (`CF_PIDMAX`).
    pid_width: bool,
    /// Whether procps-ng shows `-` for every process, because it has no
    /// value for the specifier. `ps L` leaves these specifiers out.
    pub(crate) placeholder: bool,
    /// How the column's value is aligned.
    justification: Justification,
    /// Which rows show the column.
    print: Print,
}

impl Specifier {
    /// A specifier that procps-ng has a value for.
    const fn listed(
        name: &'static str,
        header: &'static str,
        width: usize,
        print: Print,
        justification: Justification,
    ) -> Self {
        Self {
            name,
            header,
            width,
            pid_width: false,
            placeholder: false,
            justification,
            print,
        }
    }

    /// A specifier that shows `-` for every process.
    const fn placeholder(
        name: &'static str,
        header: &'static str,
        width: usize,
        print: Print,
        justification: Justification,
    ) -> Self {
        Self {
            placeholder: true,
            ..Self::listed(name, header, width, print, justification)
        }
    }

    /// The specifier, with a column as wide as the largest pid.
    const fn with_pid_width(self) -> Self {
        Self {
            pid_width: true,
            ..self
        }
    }

    /// The column's default width. A column for a pid is as wide as the
    /// largest pid or its header, whichever is wider.
    pub(crate) fn width(&self) -> usize {
        if self.pid_width {
            PID_LENGTH.max(self.header.len())
        } else {
            self.width
        }
    }
}

/// procps-ng's format specifiers, sorted by name in byte order, which
/// procps-ng searches with `bsearch`. `~` is the last specifier.
static SPECIFIERS: [Specifier; 281] = [
    Specifier::listed("%cpu", "%CPU", 4, Print::EveryTime, Justification::Right),
    Specifier::listed("%mem", "%MEM", 4, Print::ProcessOnly, Justification::Right),
    Specifier::listed(
        "_left",
        "LLLLLLLL",
        8,
        Print::EveryTime,
        Justification::Left,
    ),
    Specifier::listed(
        "_left2",
        "L2L2L2L2",
        8,
        Print::EveryTime,
        Justification::Left,
    ),
    Specifier::listed(
        "_right",
        "RRRRRRRRRRR",
        11,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed(
        "_right2",
        "R2R2R2R2R2R",
        11,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed(
        "_unlimited",
        "U",
        16,
        Print::EveryTime,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "_unlimited2",
        "U2",
        16,
        Print::EveryTime,
        Justification::Unlimited,
    ),
    Specifier::placeholder("acflag", "ACFLG", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("acflg", "ACFLG", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("addr", "ADDR", 4, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("addr_1", "ADDR", 1, Print::AsNeeded, Justification::Left),
    Specifier::listed("ag_id", "AGID", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("ag_nice", "AGNI", 4, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("alarm", "ALARM", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("argc", "ARGC", 4, Print::ProcessOnly, Justification::Right),
    Specifier::listed(
        "args",
        "COMMAND",
        27,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed("atime", "TIME", 8, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "blocked",
        "BLOCKED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::placeholder("bnd", "BND", 1, Print::ThreadOnly, Justification::Right),
    Specifier::listed(
        "bsdstart",
        "START",
        6,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed("bsdtime", "TIME", 6, Print::EveryTime, Justification::Right),
    Specifier::listed("c", "C", 2, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "caught",
        "CAUGHT",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed(
        "cgname",
        "CGNAME",
        27,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "cgroup",
        "CGROUP",
        27,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "cgroupns",
        "CGROUPNS",
        10,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed("class", "CLS", 3, Print::ThreadOnly, Justification::Left),
    Specifier::listed("cls", "CLS", 3, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("cmaj_flt", "-", 1, Print::AsNeeded, Justification::Right),
    Specifier::listed(
        "cmd",
        "CMD",
        27,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::placeholder("cmin_flt", "-", 1, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("cnswap", "-", 1, Print::AsNeeded, Justification::Right),
    Specifier::listed(
        "comm",
        "COMMAND",
        15,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "command",
        "COMMAND",
        27,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "context",
        "CONTEXT",
        31,
        Print::EveryTime,
        Justification::Left,
    ),
    Specifier::listed("cp", "CP", 3, Print::EveryTime, Justification::Right),
    Specifier::listed("cpu", "CPU", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("cpuid", "CPUID", 5, Print::ThreadOnly, Justification::Right),
    Specifier::listed("cputime", "TIME", 8, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "cputimes",
        "TIME",
        8,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::placeholder("ctid", "CTID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("cuc", "%CUC", 7, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("cursig", "CURSIG", 6, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("cutime", "-", 1, Print::AsNeeded, Justification::Right),
    Specifier::listed("cuu", "%CUU", 6, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("cwd", "CWD", 3, Print::AsNeeded, Justification::Left),
    Specifier::listed("docker", "DOCKER", 12, Print::AsNeeded, Justification::Left),
    Specifier::listed("drs", "DRS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::listed("dsiz", "DSIZ", 4, Print::ProcessOnly, Justification::Right),
    Specifier::listed("egid", "EGID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("egroup", "EGROUP", 8, Print::EveryTime, Justification::User),
    Specifier::listed("eip", "EIP", 16, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("emul", "EMUL", 13, Print::ProcessOnly, Justification::Left),
    Specifier::placeholder(
        "end_code",
        "E_CODE",
        16,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "environ",
        "ENVIRONMENT",
        31,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed("esp", "ESP", 16, Print::ThreadOnly, Justification::Right),
    Specifier::listed(
        "etime",
        "ELAPSED",
        11,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed(
        "etimes",
        "ELAPSED",
        7,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed("euid", "EUID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("euser", "EUSER", 8, Print::EveryTime, Justification::User),
    Specifier::listed(
        "exe",
        "EXE",
        27,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed("f", "F", 1, Print::EveryTime, Justification::Right),
    Specifier::listed("fds", "FDS", 3, Print::ProcessOnly, Justification::Right),
    Specifier::listed("fgid", "FGID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("fgroup", "FGROUP", 8, Print::EveryTime, Justification::User),
    Specifier::listed("flag", "F", 1, Print::EveryTime, Justification::Right),
    Specifier::listed("flags", "F", 1, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "fname",
        "COMMAND",
        8,
        Print::ProcessOnly,
        Justification::Left,
    ),
    Specifier::listed("fsgid", "FSGID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "fsgroup",
        "FSGROUP",
        8,
        Print::EveryTime,
        Justification::User,
    ),
    Specifier::listed("fsuid", "FSUID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("fsuser", "FSUSER", 8, Print::EveryTime, Justification::User),
    Specifier::listed("fuid", "FUID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("fuser", "FUSER", 8, Print::EveryTime, Justification::User),
    Specifier::listed("gid", "GID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("group", "GROUP", 8, Print::EveryTime, Justification::User),
    Specifier::listed(
        "htprv",
        "HTPRV",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "htshr",
        "HTSHR",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "ignored",
        "IGNORED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::placeholder("inblk", "INBLK", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("inblock", "INBLK", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("intpri", "PRI", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("ipcns", "IPCNS", 10, Print::EveryTime, Justification::Right),
    Specifier::placeholder("jid", "JID", 1, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("jobc", "JOBC", 4, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("ktrace", "KTRACE", 8, Print::AsNeeded, Justification::Right),
    Specifier::placeholder(
        "ktracep",
        "KTRACEP",
        8,
        Print::AsNeeded,
        Justification::Right,
    ),
    Specifier::listed("label", "LABEL", 31, Print::EveryTime, Justification::Left),
    Specifier::listed("lastcpu", "C", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("lim", "LIM", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("login", "LOGNAME", 8, Print::AsNeeded, Justification::Left),
    Specifier::placeholder(
        "logname",
        "LOGNAME",
        8,
        Print::AsNeeded,
        Justification::Left,
    ),
    Specifier::listed(
        "longtname",
        "TTY",
        8,
        Print::ProcessOnly,
        Justification::Left,
    ),
    Specifier::listed(
        "lsession",
        "SESSION",
        11,
        Print::EveryTime,
        Justification::Left,
    ),
    Specifier::listed(
        "lstart",
        "STARTED",
        24,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed("luid", "LUID", 5, Print::EveryTime, Justification::Right),
    Specifier::placeholder("luser", "LUSER", 8, Print::EveryTime, Justification::User),
    Specifier::listed("lwp", "LWP", 5, Print::ThreadOnly, Justification::Right).with_pid_width(),
    Specifier::listed("lxc", "LXC", 8, Print::EveryTime, Justification::Left),
    Specifier::listed("m_drs", "DRS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("m_dt", "DT", 4, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("m_lrs", "LRS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder(
        "m_resident",
        "RES",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder(
        "m_share",
        "SHRD",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "m_size",
        "SIZE",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder(
        "m_swap",
        "SWAP",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed("m_trs", "TRS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::listed(
        "machine",
        "MACHINE",
        31,
        Print::EveryTime,
        Justification::Left,
    ),
    Specifier::listed("maj_flt", "MAJFL", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("majflt", "MAJFLT", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("min_flt", "MINFL", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("minflt", "MINFLT", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("mntns", "MNTNS", 10, Print::EveryTime, Justification::Right),
    Specifier::placeholder("msgrcv", "MSGRCV", 6, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("msgsnd", "MSGSND", 6, Print::AsNeeded, Justification::Right),
    Specifier::placeholder(
        "mwchan",
        "MWCHAN",
        6,
        Print::ThreadOnly,
        Justification::Wchan,
    ),
    Specifier::listed("netns", "NETNS", 10, Print::EveryTime, Justification::Right),
    Specifier::listed("ni", "NI", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("nice", "NI", 3, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("nivcsw", "IVCSW", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("nlwp", "NLWP", 4, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder(
        "nsignals",
        "NSIGS",
        5,
        Print::AsNeeded,
        Justification::Right,
    ),
    Specifier::placeholder("nsigs", "NSIGS", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("nswap", "NSWAP", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("numa", "NUMA", 4, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("nvcsw", "VCSW", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder(
        "nwchan",
        "WCHAN",
        6,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed("oom", "OOM", 4, Print::ThreadOnly, Justification::Right),
    Specifier::listed(
        "oomadj",
        "OOMADJ",
        5,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed("opri", "PRI", 3, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("osz", "SZ", 2, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("oublk", "OUBLK", 5, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("oublock", "OUBLK", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("ouid", "OWNER", 5, Print::EveryTime, Justification::Left),
    Specifier::placeholder("p_ru", "P_RU", 6, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("paddr", "PADDR", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("pagein", "PAGEIN", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("pcap", "PCAP", 16, Print::ThreadOnly, Justification::Right),
    Specifier::listed(
        "pcaps",
        "PCAPS",
        16,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed("pcpu", "%CPU", 4, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "pending",
        "PENDING",
        9,
        Print::EveryTime,
        Justification::Signal,
    ),
    Specifier::listed("pgid", "PGID", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed("pgrp", "PGRP", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed("pid", "PID", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed("pidns", "PIDNS", 10, Print::EveryTime, Justification::Right),
    Specifier::listed("pmem", "%MEM", 4, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("poip", "-", 1, Print::AsNeeded, Justification::Right),
    Specifier::listed("policy", "POL", 3, Print::ThreadOnly, Justification::Left),
    Specifier::listed("ppid", "PPID", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed("pri", "PRI", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("pri_api", "API", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("pri_bar", "BAR", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("pri_baz", "BAZ", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("pri_foo", "FOO", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed(
        "priority",
        "PRI",
        3,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::placeholder(
        "prmgrp",
        "PRMGRP",
        12,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder(
        "prmid",
        "PRMID",
        12,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder(
        "project",
        "PROJECT",
        12,
        Print::ProcessOnly,
        Justification::Left,
    ),
    Specifier::placeholder(
        "projid",
        "PROJID",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder("pset", "PSET", 4, Print::ThreadOnly, Justification::Right),
    Specifier::listed("psr", "PSR", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("pss", "PSS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("psxpri", "PPR", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed(
        "rbytes",
        "RBYTES",
        5,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "rchars",
        "RCHARS",
        5,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::placeholder("re", "RE", 3, Print::AsNeeded, Justification::Right),
    Specifier::placeholder(
        "resident",
        "RES",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed("rgid", "RGID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("rgroup", "RGROUP", 8, Print::EveryTime, Justification::User),
    Specifier::placeholder("rlink", "RLINK", 8, Print::AsNeeded, Justification::Right),
    Specifier::listed("rops", "ROPS", 5, Print::ThreadOnly, Justification::Right),
    Specifier::listed("rss", "RSS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::listed("rssize", "RSS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::listed("rsz", "RSZ", 5, Print::ProcessOnly, Justification::Right),
    Specifier::listed(
        "rtprio",
        "RTPRIO",
        6,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed("ruid", "RUID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("ruser", "RUSER", 8, Print::EveryTime, Justification::User),
    Specifier::listed("s", "S", 1, Print::ThreadOnly, Justification::Left),
    Specifier::listed("sched", "SCH", 3, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("scnt", "SCNT", 4, Print::AsNeeded, Justification::Right),
    Specifier::placeholder("scount", "SC", 4, Print::AsNeeded, Justification::Right),
    Specifier::listed("seat", "SEAT", 11, Print::EveryTime, Justification::Left),
    Specifier::listed("sess", "SESS", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed(
        "session",
        "SESS",
        5,
        Print::ProcessOnly,
        Justification::Right,
    )
    .with_pid_width(),
    Specifier::listed("sgi_p", "P", 1, Print::ThreadOnly, Justification::Right),
    Specifier::listed("sgi_rss", "RSS", 4, Print::ProcessOnly, Justification::Left),
    Specifier::listed("sgid", "SGID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("sgroup", "SGROUP", 8, Print::EveryTime, Justification::User),
    Specifier::placeholder("share", "-", 1, Print::ProcessOnly, Justification::Right),
    Specifier::listed("sid", "SID", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed("sig", "PENDING", 9, Print::EveryTime, Justification::Signal),
    Specifier::listed(
        "sig_block",
        "BLOCKED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed(
        "sig_catch",
        "CATCHED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed(
        "sig_ignore",
        "IGNORED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed(
        "sig_pend",
        "SIGNAL",
        9,
        Print::EveryTime,
        Justification::Signal,
    ),
    Specifier::listed(
        "sigcatch",
        "CAUGHT",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed(
        "sigignore",
        "IGNORED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed(
        "sigmask",
        "BLOCKED",
        9,
        Print::ThreadOnly,
        Justification::Signal,
    ),
    Specifier::listed("size", "SIZE", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("sl", "SL", 3, Print::AsNeeded, Justification::Right),
    Specifier::listed("slice", "SLICE", 31, Print::EveryTime, Justification::Left),
    Specifier::listed("spid", "SPID", 5, Print::ThreadOnly, Justification::Right).with_pid_width(),
    Specifier::listed(
        "stackp",
        "STACKP",
        16,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "start",
        "STARTED",
        8,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::placeholder(
        "start_code",
        "S_CODE",
        16,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "start_stack",
        "STACKP",
        16,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "start_time",
        "START",
        5,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed("stat", "STAT", 4, Print::ThreadOnly, Justification::Left),
    Specifier::listed("state", "S", 1, Print::ThreadOnly, Justification::Left),
    Specifier::placeholder("status", "STATUS", 6, Print::AsNeeded, Justification::Right),
    Specifier::listed("stime", "STIME", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("suid", "SUID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "supgid",
        "SUPGID",
        20,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "supgrp",
        "SUPGRP",
        40,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed("suser", "SUSER", 8, Print::EveryTime, Justification::User),
    Specifier::listed("svgid", "SVGID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "svgroup",
        "SVGROUP",
        8,
        Print::EveryTime,
        Justification::User,
    ),
    Specifier::listed("svuid", "SVUID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("svuser", "SVUSER", 8, Print::EveryTime, Justification::User),
    Specifier::placeholder(
        "systime",
        "SYSTEM",
        6,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::listed("sz", "SZ", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder(
        "taskid",
        "TASKID",
        5,
        Print::ThreadOnly,
        Justification::Right,
    )
    .with_pid_width(),
    Specifier::placeholder("tdev", "TDEV", 4, Print::AsNeeded, Justification::Right),
    Specifier::listed("tgid", "TGID", 5, Print::ProcessOnly, Justification::Right).with_pid_width(),
    Specifier::listed(
        "thcount",
        "THCNT",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed("tid", "TID", 5, Print::ThreadOnly, Justification::Right).with_pid_width(),
    Specifier::listed("time", "TIME", 8, Print::EveryTime, Justification::Right),
    Specifier::listed(
        "timens",
        "TIMENS",
        10,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::placeholder("timeout", "TMOUT", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("times", "TIME", 8, Print::EveryTime, Justification::Right),
    Specifier::placeholder("tmout", "TMOUT", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("tname", "TTY", 8, Print::ProcessOnly, Justification::Left),
    Specifier::listed(
        "tpgid",
        "TPGID",
        5,
        Print::ProcessOnly,
        Justification::Right,
    )
    .with_pid_width(),
    Specifier::listed("trs", "TRS", 4, Print::ProcessOnly, Justification::Right),
    Specifier::listed("trss", "TRSS", 4, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder(
        "tsess",
        "TSESS",
        5,
        Print::ProcessOnly,
        Justification::Right,
    )
    .with_pid_width(),
    Specifier::placeholder(
        "tsession",
        "TSESS",
        5,
        Print::ProcessOnly,
        Justification::Right,
    )
    .with_pid_width(),
    Specifier::placeholder("tsid", "TSID", 5, Print::ProcessOnly, Justification::Right)
        .with_pid_width(),
    Specifier::listed(
        "tsig",
        "PENDING",
        9,
        Print::EveryTime,
        Justification::Signal,
    ),
    Specifier::listed("tsiz", "TSIZ", 4, Print::ProcessOnly, Justification::Right),
    Specifier::listed("tt", "TT", 8, Print::ProcessOnly, Justification::Left),
    Specifier::listed("tty", "TT", 8, Print::ProcessOnly, Justification::Left),
    Specifier::listed("tty4", "TTY", 4, Print::ProcessOnly, Justification::Left),
    Specifier::listed("tty8", "TTY", 8, Print::ProcessOnly, Justification::Left),
    Specifier::placeholder(
        "u_procp",
        "UPROCP",
        6,
        Print::AsNeeded,
        Justification::Right,
    ),
    Specifier::listed(
        "ucmd",
        "CMD",
        15,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed(
        "ucomm",
        "COMMAND",
        15,
        Print::ProcessOnly,
        Justification::Unlimited,
    ),
    Specifier::listed("uid", "UID", 5, Print::EveryTime, Justification::Right),
    Specifier::listed("uid_hack", "UID", 8, Print::EveryTime, Justification::User),
    Specifier::placeholder("umask", "UMASK", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("uname", "USER", 8, Print::EveryTime, Justification::User),
    Specifier::listed("unit", "UNIT", 31, Print::EveryTime, Justification::Left),
    Specifier::placeholder("upr", "UPR", 3, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("uprocp", "UPROCP", 8, Print::AsNeeded, Justification::Right),
    Specifier::listed("user", "USER", 8, Print::EveryTime, Justification::User),
    Specifier::listed(
        "userns",
        "USERNS",
        10,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::placeholder(
        "usertime",
        "USER",
        4,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::placeholder("usrpri", "UPR", 3, Print::ThreadOnly, Justification::Right),
    Specifier::listed("uss", "USS", 5, Print::ProcessOnly, Justification::Right),
    Specifier::listed("util", "C", 2, Print::EveryTime, Justification::Right),
    Specifier::placeholder("utime", "UTIME", 6, Print::EveryTime, Justification::Right),
    Specifier::listed("utsns", "UTSNS", 10, Print::EveryTime, Justification::Right),
    Specifier::listed("uunit", "UUNIT", 31, Print::EveryTime, Justification::Left),
    Specifier::placeholder(
        "vm_data",
        "DATA",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder("vm_exe", "EXE", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder("vm_lib", "LIB", 5, Print::ProcessOnly, Justification::Right),
    Specifier::placeholder(
        "vm_lock",
        "LCK",
        3,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::placeholder(
        "vm_stack",
        "STACK",
        5,
        Print::ProcessOnly,
        Justification::Right,
    ),
    Specifier::listed("vsize", "VSZ", 6, Print::ProcessOnly, Justification::Right),
    Specifier::listed("vsz", "VSZ", 6, Print::ProcessOnly, Justification::Right),
    Specifier::listed(
        "wbytes",
        "WBYTES",
        5,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed(
        "wcbytes",
        "WCBYTES",
        5,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed("wchan", "WCHAN", 6, Print::ThreadOnly, Justification::Wchan),
    Specifier::listed(
        "wchars",
        "WCHARS",
        5,
        Print::ThreadOnly,
        Justification::Right,
    ),
    Specifier::listed("wname", "WCHAN", 6, Print::ThreadOnly, Justification::Wchan),
    Specifier::listed("wops", "WOPS", 5, Print::ThreadOnly, Justification::Right),
    Specifier::placeholder("xstat", "XSTAT", 5, Print::AsNeeded, Justification::Right),
    Specifier::listed("zone", "ZONE", 31, Print::EveryTime, Justification::Left),
    Specifier::placeholder(
        "zoneid",
        "ZONEID",
        31,
        Print::EveryTime,
        Justification::Right,
    ),
    Specifier::placeholder("~", "-", 1, Print::AsNeeded, Justification::Right),
];

/// The format macros and their expansions, which list specifiers separated by
/// commas, sorted by the macro's name in byte order.
static MACROS: [(&str, &str); 49] = [
    ("DFMT", "pid,tname,state,cputime,cmd"),
    ("DefBSD", "pid,tname,stat,bsdtime,args"),
    ("DefSysV", "pid,tname,time,cmd"),
    ("END_BSD", "state,tname,cputime,comm"),
    ("END_SYS5", "state,tname,time,command"),
    ("F5FMT", "uname,pid,ppid,c,start,tname,time,cmd"),
    ("FB_", "pid,tt,stat,time,command"),
    ("FB_j", "user,pid,ppid,pgid,sess,jobc,stat,tt,time,command"),
    (
        "FB_l",
        "uid,pid,ppid,cpu,pri,nice,vsz,rss,wchan,stat,tt,time,command",
    ),
    (
        "FB_u",
        "user,pid,pcpu,pmem,vsz,rss,tt,stat,start,time,command",
    ),
    (
        "FB_v",
        "pid,stat,time,sl,re,pagein,vsz,rss,lim,tsiz,pcpu,pmem,command",
    ),
    ("FD_", "pid,tty,time,comm"),
    ("FD_f", "user,pid,ppid,start_time,tty,time,comm"),
    ("FD_fj", "user,pid,ppid,start_time,tty,time,pgid,sid,comm"),
    ("FD_j", "pid,tty,time,pgid,sid,comm"),
    (
        "FD_l",
        "flags,state,uid,pid,ppid,priority,nice,vsz,wchan,tty,time,comm",
    ),
    (
        "FD_lj",
        "flags,state,uid,pid,ppid,priority,nice,vsz,wchan,tty,time,pgid,sid,comm",
    ),
    (
        "FL5FMT",
        "f,state,uid,pid,ppid,pcpu,pri,nice,rss,wchan,start,time,command",
    ),
    ("FLASK_context", "pid,context,command"),
    ("HP_", "pid,tty,time,comm"),
    ("HP_f", "user,pid,ppid,cpu,stime,tty,time,args"),
    (
        "HP_fl",
        "flags,state,user,pid,ppid,cpu,intpri,nice,addr,sz,wchan,stime,tty,time,args",
    ),
    (
        "HP_l",
        "flags,state,uid,pid,ppid,cpu,intpri,nice,addr,sz,wchan,tty,time,comm",
    ),
    ("J390", "pid,sid,pgrp,tname,atime,args"),
    (
        "JFMT",
        "user,pid,ppid,pgid,sess,jobc,state,tname,cputime,command",
    ),
    (
        "L5FMT",
        "f,state,uid,pid,ppid,c,pri,nice,addr,sz,wchan,tt,time,ucmd",
    ),
    (
        "LFMT",
        "uid,pid,ppid,cp,pri,nice,vsz,rss,wchan,state,tname,cputime,command",
    ),
    (
        "OL_X",
        "pid,start_stack,esp,eip,timeout,alarm,stat,tname,bsdtime,args",
    ),
    (
        "OL_j",
        "ppid,pid,pgid,sid,tname,tpgid,stat,uid,bsdtime,args",
    ),
    (
        "OL_l",
        "flags,uid,pid,ppid,priority,nice,vsz,rss,wchan,stat,tname,bsdtime,args",
    ),
    (
        "OL_m",
        "pid,tname,majflt,minflt,m_trs,m_drs,m_size,m_swap,rss,m_share,vm_lib,m_dt,args",
    ),
    (
        "OL_s",
        "uid,pid,pending,sig_block,sig_ignore,caught,stat,tname,bsdtime,args",
    ),
    (
        "OL_u",
        "user,pid,pcpu,pmem,vsz,rss,tname,stat,start_time,bsdtime,args",
    ),
    (
        "OL_v",
        "pid,tname,stat,bsdtime,maj_flt,m_trs,m_drs,rss,pmem,args",
    ),
    ("RD_", "pid,tname,state,bsdtime,comm"),
    ("RD_f", "uid,pid,ppid,start_time,tname,bsdtime,args"),
    (
        "RD_fj",
        "uid,pid,ppid,start_time,tname,bsdtime,pgid,sid,args",
    ),
    ("RD_j", "pid,tname,state,bsdtime,pgid,sid,comm"),
    (
        "RD_l",
        "flags,state,uid,pid,ppid,priority,nice,wchan,tname,bsdtime,comm",
    ),
    (
        "RD_lj",
        "flags,state,uid,pid,ppid,priority,nice,wchan,tname,bsdtime,pgid,sid,comm",
    ),
    (
        "RUSAGE",
        "minflt,majflt,nswap,inblock,oublock,msgsnd,msgrcv,nsigs,nvcsw,nivcsw",
    ),
    ("SCHED", "user,pcpu,pri,usrpri,nice,psxpri,psr,policy,pset"),
    (
        "SFMT",
        "uid,pid,cursig,sig,sigmask,sigignore,sigcatch,stat,tname,command",
    ),
    ("Std_f", "uid_hack,pid,ppid,c,stime,tname,time,cmd"),
    (
        "Std_fl",
        "f,s,uid_hack,pid,ppid,c,opri,ni,addr,sz,wchan,stime,tname,time,cmd",
    ),
    (
        "Std_l",
        "f,s,uid,pid,ppid,c,opri,ni,addr,sz,wchan,tname,time,ucmd",
    ),
    ("THREAD", "user,pcpu,pri,scnt,wchan,usertime,systime"),
    (
        "UFMT",
        "uname,pid,pcpu,pmem,vsz,rss,tt,state,start,time,command",
    ),
    (
        "VFMT",
        "pid,tt,state,time,sl,pagein,vsz,rss,pcpu,pmem,command",
    ),
];

/// The AIX field descriptors, such as `%p`, with the specifier and the header
/// for each.
static AIX_DESCRIPTORS: [(u8, &str, &str); 15] = [
    (b'C', "pcpu", "%CPU"),
    (b'G', "group", "GROUP"),
    (b'P', "ppid", "PPID"),
    (b'U', "user", "USER"),
    (b'a', "args", "COMMAND"),
    (b'c', "comm", "COMMAND"),
    (b'g', "rgroup", "RGROUP"),
    (b'n', "nice", "NI"),
    (b'p', "pid", "PID"),
    (b'r', "pgid", "PGID"),
    (b't', "etime", "ELAPSED"),
    (b'u', "ruser", "RUSER"),
    (b'x', "time", "TIME"),
    (b'y', "tty", "TTY"),
    (b'z', "vsz", "VSZ"),
];

/// The codes of BSD's `O` sort option, with the specifier for each. `K`
/// passes procps-ng's check of the codes but has no specifier.
static SORT_CODES: [(u8, &str); 25] = [
    (b'C', "pcpu"),
    (b'G', "tpgid"),
    (b'J', "cstime"),
    (b'M', "maj_flt"),
    (b'N', "cmaj_flt"),
    (b'P', "ppid"),
    (b'R', "resident"),
    (b'S', "share"),
    (b'T', "start_time"),
    (b'U', "uid"),
    (b'c', "cmd"),
    (b'f', "flags"),
    (b'g', "pgrp"),
    (b'j', "cutime"),
    (b'k', "utime"),
    (b'm', "min_flt"),
    (b'n', "cmin_flt"),
    (b'o', "session"),
    (b'p', "pid"),
    (b'r', "rss"),
    (b's', "size"),
    (b't', "tty"),
    (b'u', "user"),
    (b'v', "vsize"),
    (b'y', "priority"),
];

/// The specifier with `name`, found as procps-ng's `search_format_array`
/// finds it.
pub(crate) fn find(name: &[u8]) -> Option<&'static Specifier> {
    SPECIFIERS
        .binary_search_by(|specifier| specifier.name.as_bytes().cmp(name))
        .ok()
        .map(|index| &SPECIFIERS[index])
}

/// The expansion of the format macro `name`, which lists specifiers separated
/// by commas.
pub(crate) fn find_macro(name: &[u8]) -> Option<&'static str> {
    MACROS
        .binary_search_by(|(macro_name, _)| macro_name.as_bytes().cmp(name))
        .ok()
        .map(|index| MACROS[index].1)
}

/// The specifier and the header for the AIX field descriptor `code`.
pub(crate) fn aix_descriptor(code: u8) -> Option<(&'static str, &'static str)> {
    AIX_DESCRIPTORS
        .iter()
        .find(|(descriptor, ..)| *descriptor == code)
        .map(|&(_, specifier, header)| (specifier, header))
}

/// The specifier for the code `code` of BSD's `O` sort option.
pub(crate) fn sort_code(code: u8) -> Option<&'static str> {
    SORT_CODES
        .iter()
        .find(|(sort_code, _)| *sort_code == code)
        .map(|&(_, specifier)| specifier)
}

/// The lines that `ps L` prints: one for each specifier that procps-ng has a
/// value for, in order, before `~`, with its name and header cut to 12 and 8
/// bytes and padded to those widths, as procps-ng's `print_format_specifiers`
/// prints them.
pub(crate) fn listing() -> Vec<Vec<u8>> {
    let field = |text: &str, width: usize| {
        let text = &text.as_bytes()[..text.len().min(width)];
        let padding = width - text.len();

        [text, &b" ".repeat(padding)].concat()
    };

    SPECIFIERS
        .iter()
        .take_while(|specifier| specifier.name != "~")
        .filter(|specifier| !specifier.placeholder)
        .map(|specifier| {
            [
                field(specifier.name, 12),
                b" ".to_vec(),
                field(specifier.header, 8),
                b"\n".to_vec(),
            ]
            .concat()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{
        AIX_DESCRIPTORS, MACROS, SPECIFIERS, aix_descriptor, find, find_macro, listing, sort_code,
    };

    #[test]
    fn the_tables_are_sorted_for_a_binary_search() {
        let specifiers: Vec<&str> = SPECIFIERS.iter().map(|specifier| specifier.name).collect();
        let macros: Vec<&str> = MACROS.iter().map(|(name, _)| *name).collect();
        let mut sorted = (specifiers.clone(), macros.clone());

        sorted.0.sort_unstable();
        sorted.0.dedup();
        sorted.1.sort_unstable();
        sorted.1.dedup();

        assert_eq!((specifiers, macros), sorted);
    }

    #[test]
    fn every_macro_and_descriptor_names_known_specifiers() {
        let unknown: Vec<&str> = MACROS
            .iter()
            .flat_map(|(_, specifiers)| specifiers.split(','))
            .chain(AIX_DESCRIPTORS.iter().map(|&(_, specifier, _)| specifier))
            .filter(|name| find(name.as_bytes()).is_none())
            .collect();

        assert_eq!(unknown, Vec::<&str>::new());
    }

    #[rstest]
    #[case::pid("pid", Some(("PID", 5, false)))]
    #[case::pid_header_wider("taskid", Some(("TASKID", 6, true)))]
    #[case::signal("caught", Some(("CAUGHT", 9, false)))]
    #[case::register("eip", Some(("EIP", 16, false)))]
    #[case::placeholder("alarm", Some(("ALARM", 5, true)))]
    #[case::last("~", Some(("-", 1, true)))]
    #[case::macro_name("DefBSD", None)]
    #[case::prefix("pi", None)]
    fn a_specifier_has_procps_ng_s_header_and_width(
        #[case] name: &str,
        #[case] expected: Option<(&str, usize, bool)>,
    ) {
        let found = find(name.as_bytes())
            .map(|specifier| (specifier.header, specifier.width(), specifier.placeholder));

        assert_eq!(found, expected);
    }

    #[test]
    fn macros_aix_descriptors_and_sort_codes_are_found_by_name() {
        assert_eq!(
            (
                find_macro(b"DefSysV"),
                find_macro(b"pid"),
                aix_descriptor(b'p'),
                aix_descriptor(b'q'),
                sort_code(b'T'),
                sort_code(b'K'),
            ),
            (
                Some("pid,tname,time,cmd"),
                None,
                Some(("pid", "PID")),
                None,
                Some("start_time"),
                None,
            )
        );
    }

    #[test]
    fn the_listing_has_procps_ng_s_layout() {
        let listing = String::from_utf8(listing().concat()).expect("the listing is ASCII");
        let lines: Vec<&str> = listing.lines().collect();

        assert_eq!(
            (lines.len(), &lines[..3], lines[39], lines.last().copied()),
            (
                198,
                &[
                    "%cpu         %CPU    ",
                    "%mem         %MEM    ",
                    "_left        LLLLLLLL"
                ][..],
                "environ      ENVIRONM",
                Some("zone         ZONE    ")
            )
        );
    }
}
