// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2015-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2011 Lukas Nykryn <lnykryn@redhat.com>
// SPDX-FileCopyrightText: 1999-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/output.c.

use crate::ps::{print::Printer, sort::Item};

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
    /// How the column shows a process's value.
    pub(crate) printer: Printer,
    /// The value that the specifier sorts by.
    pub(crate) sort: Item,
    /// The width, unless the column shows a pid.
    width: usize,
    /// Whether the column shows a pid and is as wide as the largest pid
    /// (`CF_PIDMAX`).
    pid_width: bool,
    /// How the column's value is aligned.
    pub(crate) justification: Justification,
    /// Which rows show the column.
    pub(crate) print: Print,
}

impl Specifier {
    /// A specifier, with procps-ng's values in the order of its table.
    const fn new(
        name: &'static str,
        header: &'static str,
        printer: Printer,
        sort: Item,
        width: usize,
        print: Print,
        justification: Justification,
    ) -> Self {
        Self {
            name,
            header,
            printer,
            sort,
            width,
            pid_width: false,
            justification,
            print,
        }
    }

    /// The specifier, with a column as wide as the largest pid.
    const fn with_pid_width(self) -> Self {
        Self {
            pid_width: true,
            ..self
        }
    }

    /// Whether procps-ng shows `-` for every process, because it has no
    /// value for the specifier. `ps L` leaves these specifiers out.
    pub(crate) fn is_placeholder(&self) -> bool {
        self.printer == Printer::Nop
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
/// procps-ng searches with `bsearch`. `~` is the last specifier. The rows
/// follow procps-ng's table, with its names for the alignments and for the
/// rows that show each column.
#[rustfmt::skip]
static SPECIFIERS: [Specifier; 281] = {
    use Justification::{
        Left as LEFT, Right as RIGHT, Signal as SIGNAL, Unlimited as UNLIMITED, User as USER,
        Wchan as WCHAN,
    };
    use Print::{AsNeeded as AN, EveryTime as ET, ProcessOnly as PO, ThreadOnly as TO};

    [
        Specifier::new("%cpu",        "%CPU",        Printer::Pcpu,         Item::Utilization,    4,  ET, RIGHT),
        Specifier::new("%mem",        "%MEM",        Printer::Pmem,         Item::VmRss,          4,  PO, RIGHT),
        Specifier::new("_left",       "LLLLLLLL",    Printer::TLeft,        Item::Noop,           8,  ET, LEFT),
        Specifier::new("_left2",      "L2L2L2L2",    Printer::TLeft2,       Item::Noop,           8,  ET, LEFT),
        Specifier::new("_right",      "RRRRRRRRRRR", Printer::TRight,       Item::Noop,           11, ET, RIGHT),
        Specifier::new("_right2",     "R2R2R2R2R2R", Printer::TRight2,      Item::Noop,           11, ET, RIGHT),
        Specifier::new("_unlimited",  "U",           Printer::TUnlimited,   Item::Noop,           16, ET, UNLIMITED),
        Specifier::new("_unlimited2", "U2",          Printer::TUnlimited2,  Item::Noop,           16, ET, UNLIMITED),
        Specifier::new("acflag",      "ACFLG",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("acflg",       "ACFLG",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("addr",        "ADDR",        Printer::Nop,          Item::Noop,           4,  AN, RIGHT),
        Specifier::new("addr_1",      "ADDR",        Printer::Nop,          Item::Noop,           1,  AN, LEFT),
        Specifier::new("ag_id",       "AGID",        Printer::Agid,         Item::AutogrpId,      5,  AN, RIGHT),
        Specifier::new("ag_nice",     "AGNI",        Printer::Agnice,       Item::AutogrpNice,    4,  AN, RIGHT),
        Specifier::new("alarm",       "ALARM",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("argc",        "ARGC",        Printer::Nop,          Item::Noop,           4,  PO, RIGHT),
        Specifier::new("args",        "COMMAND",     Printer::Args,         Item::Cmdline,        27, PO, UNLIMITED),
        Specifier::new("atime",       "TIME",        Printer::Time,         Item::TimeAll,        8,  ET, RIGHT),
        Specifier::new("blocked",     "BLOCKED",     Printer::Sigmask,      Item::Sigblocked,     9,  TO, SIGNAL),
        Specifier::new("bnd",         "BND",         Printer::Nop,          Item::Noop,           1,  TO, RIGHT),
        Specifier::new("bsdstart",    "START",       Printer::Bsdstart,     Item::TicsBegan,      6,  ET, RIGHT),
        Specifier::new("bsdtime",     "TIME",        Printer::Bsdtime,      Item::TicsAll,        6,  ET, RIGHT),
        Specifier::new("c",           "C",           Printer::C,            Item::Utilization,    2,  ET, RIGHT),
        Specifier::new("caught",      "CAUGHT",      Printer::Sigcatch,     Item::Sigcatch,       9,  TO, SIGNAL),
        Specifier::new("cgname",      "CGNAME",      Printer::Cgname,       Item::Cgname,         27, PO, UNLIMITED),
        Specifier::new("cgroup",      "CGROUP",      Printer::Cgroup,       Item::Cgroup,         27, PO, UNLIMITED),
        Specifier::new("cgroupns",    "CGROUPNS",    Printer::Cgroupns,     Item::NsCgroup,       10, ET, RIGHT),
        Specifier::new("class",       "CLS",         Printer::Class,        Item::SchedClassstr,  3,  TO, LEFT),
        Specifier::new("cls",         "CLS",         Printer::Class,        Item::SchedClassstr,  3,  TO, RIGHT),
        Specifier::new("cmaj_flt",    "-",           Printer::Nop,          Item::Noop,           1,  AN, RIGHT),
        Specifier::new("cmd",         "CMD",         Printer::Args,         Item::Cmdline,        27, PO, UNLIMITED),
        Specifier::new("cmin_flt",    "-",           Printer::Nop,          Item::Noop,           1,  AN, RIGHT),
        Specifier::new("cnswap",      "-",           Printer::Nop,          Item::Noop,           1,  AN, RIGHT),
        Specifier::new("comm",        "COMMAND",     Printer::Comm,         Item::Cmd,            15, PO, UNLIMITED),
        Specifier::new("command",     "COMMAND",     Printer::Args,         Item::Cmdline,        27, PO, UNLIMITED),
        Specifier::new("context",     "CONTEXT",     Printer::Context,      Item::IdTgid,         31, ET, LEFT),
        Specifier::new("cp",          "CP",          Printer::Cp,           Item::Utilization,    3,  ET, RIGHT),
        Specifier::new("cpu",         "CPU",         Printer::Psr,          Item::Processor,      3,  TO, RIGHT),
        Specifier::new("cpuid",       "CPUID",       Printer::Psr,          Item::Processor,      5,  TO, RIGHT),
        Specifier::new("cputime",     "TIME",        Printer::Time,         Item::TimeAll,        8,  ET, RIGHT),
        Specifier::new("cputimes",    "TIME",        Printer::Times,        Item::TimeAll,        8,  ET, RIGHT),
        Specifier::new("ctid",        "CTID",        Printer::Nop,          Item::Noop,           5,  ET, RIGHT),
        Specifier::new("cuc",         "%CUC",        Printer::UtilizationC, Item::UtilizationC,   7,  AN, RIGHT),
        Specifier::new("cursig",      "CURSIG",      Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("cutime",      "-",           Printer::Nop,          Item::TicsUserC,      1,  AN, RIGHT),
        Specifier::new("cuu",         "%CUU",        Printer::Utilization,  Item::Utilization,    6,  AN, RIGHT),
        Specifier::new("cwd",         "CWD",         Printer::Nop,          Item::Noop,           3,  AN, LEFT),
        Specifier::new("docker",      "DOCKER",      Printer::Docker,       Item::DockerId,       12, AN, LEFT),
        Specifier::new("drs",         "DRS",         Printer::Drs,          Item::VsizeBytes,     5,  PO, RIGHT),
        Specifier::new("dsiz",        "DSIZ",        Printer::Dsiz,         Item::VsizeBytes,     4,  PO, RIGHT),
        Specifier::new("egid",        "EGID",        Printer::Egid,         Item::IdEgid,         5,  ET, RIGHT),
        Specifier::new("egroup",      "EGROUP",      Printer::Egroup,       Item::IdEgroup,       8,  ET, USER),
        Specifier::new("eip",         "EIP",         Printer::Eip,          Item::AddrCurrEip,    16, TO, RIGHT),
        Specifier::new("emul",        "EMUL",        Printer::Nop,          Item::Noop,           13, PO, LEFT),
        Specifier::new("end_code",    "E_CODE",      Printer::Nop,          Item::AddrCodeEnd,    16, PO, RIGHT),
        Specifier::new("environ",     "ENVIRONMENT", Printer::Environ,      Item::Noop,           31, PO, UNLIMITED),
        Specifier::new("esp",         "ESP",         Printer::Esp,          Item::AddrCurrEsp,    16, TO, RIGHT),
        Specifier::new("etime",       "ELAPSED",     Printer::Etime,        Item::TimeElapsed,    11, ET, RIGHT),
        Specifier::new("etimes",      "ELAPSED",     Printer::Etimes,       Item::TimeElapsed,    7,  ET, RIGHT),
        Specifier::new("euid",        "EUID",        Printer::Euid,         Item::IdEuid,         5,  ET, RIGHT),
        Specifier::new("euser",       "EUSER",       Printer::Euser,        Item::IdEuser,        8,  ET, USER),
        Specifier::new("exe",         "EXE",         Printer::Exe,          Item::Exe,            27, PO, UNLIMITED),
        Specifier::new("f",           "F",           Printer::Flag,         Item::Flags,          1,  ET, RIGHT),
        Specifier::new("fds",         "FDS",         Printer::Fds,          Item::OpenFiles,      3,  PO, RIGHT),
        Specifier::new("fgid",        "FGID",        Printer::Fgid,         Item::Flags,          5,  ET, RIGHT),
        Specifier::new("fgroup",      "FGROUP",      Printer::Fgroup,       Item::IdFgroup,       8,  ET, USER),
        Specifier::new("flag",        "F",           Printer::Flag,         Item::Flags,          1,  ET, RIGHT),
        Specifier::new("flags",       "F",           Printer::Flag,         Item::Flags,          1,  ET, RIGHT),
        Specifier::new("fname",       "COMMAND",     Printer::Fname,        Item::Cmd,            8,  PO, LEFT),
        Specifier::new("fsgid",       "FSGID",       Printer::Fgid,         Item::IdFgid,         5,  ET, RIGHT),
        Specifier::new("fsgroup",     "FSGROUP",     Printer::Fgroup,       Item::IdFgroup,       8,  ET, USER),
        Specifier::new("fsuid",       "FSUID",       Printer::Fuid,         Item::IdFuid,         5,  ET, RIGHT),
        Specifier::new("fsuser",      "FSUSER",      Printer::Fuser,        Item::IdFuser,        8,  ET, USER),
        Specifier::new("fuid",        "FUID",        Printer::Fuid,         Item::IdFuid,         5,  ET, RIGHT),
        Specifier::new("fuser",       "FUSER",       Printer::Fuser,        Item::IdFuser,        8,  ET, USER),
        Specifier::new("gid",         "GID",         Printer::Egid,         Item::IdEgid,         5,  ET, RIGHT),
        Specifier::new("group",       "GROUP",       Printer::Egroup,       Item::IdEgroup,       8,  ET, USER),
        Specifier::new("htprv",       "HTPRV",       Printer::Hugetblprv,   Item::SmapHugeTlbprv, 5,  PO, RIGHT),
        Specifier::new("htshr",       "HTSHR",       Printer::Hugetblshr,   Item::SmapHugeTlbprv, 5,  PO, RIGHT),
        Specifier::new("ignored",     "IGNORED",     Printer::Sigignore,    Item::Sigignore,      9,  TO, SIGNAL),
        Specifier::new("inblk",       "INBLK",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("inblock",     "INBLK",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("intpri",      "PRI",         Printer::Opri,         Item::Priority,       3,  TO, RIGHT),
        Specifier::new("ipcns",       "IPCNS",       Printer::Ipcns,        Item::NsIpc,          10, ET, RIGHT),
        Specifier::new("jid",         "JID",         Printer::Nop,          Item::Noop,           1,  PO, RIGHT),
        Specifier::new("jobc",        "JOBC",        Printer::Nop,          Item::Noop,           4,  AN, RIGHT),
        Specifier::new("ktrace",      "KTRACE",      Printer::Nop,          Item::Noop,           8,  AN, RIGHT),
        Specifier::new("ktracep",     "KTRACEP",     Printer::Nop,          Item::Noop,           8,  AN, RIGHT),
        Specifier::new("label",       "LABEL",       Printer::Context,      Item::IdTgid,         31, ET, LEFT),
        Specifier::new("lastcpu",     "C",           Printer::Psr,          Item::Processor,      3,  TO, RIGHT),
        Specifier::new("lim",         "LIM",         Printer::Lim,          Item::RssRlim,        5,  AN, RIGHT),
        Specifier::new("login",       "LOGNAME",     Printer::Nop,          Item::Noop,           8,  AN, LEFT),
        Specifier::new("logname",     "LOGNAME",     Printer::Nop,          Item::Noop,           8,  AN, LEFT),
        Specifier::new("longtname",   "TTY",         Printer::Tty8,         Item::TtyName,        8,  PO, LEFT),
        Specifier::new("lsession",    "SESSION",     Printer::SdSession,    Item::SdSess,         11, ET, LEFT),
        Specifier::new("lstart",      "STARTED",     Printer::Lstart,       Item::TicsBegan,      24, ET, RIGHT),
        Specifier::new("luid",        "LUID",        Printer::Luid,         Item::IdLogin,        5,  ET, RIGHT),
        Specifier::new("luser",       "LUSER",       Printer::Nop,          Item::Noop,           8,  ET, USER),
        Specifier::new("lwp",         "LWP",         Printer::Tasks,        Item::IdPid,          5,  TO, RIGHT).with_pid_width(),
        Specifier::new("lxc",         "LXC",         Printer::Lxcname,      Item::Lxcname,        8,  ET, LEFT),
        Specifier::new("m_drs",       "DRS",         Printer::Drs,          Item::VsizeBytes,     5,  PO, RIGHT),
        Specifier::new("m_dt",        "DT",          Printer::Nop,          Item::Noop,           4,  PO, RIGHT),
        Specifier::new("m_lrs",       "LRS",         Printer::Nop,          Item::Noop,           5,  PO, RIGHT),
        Specifier::new("m_resident",  "RES",         Printer::Nop,          Item::MemResPgs,      5,  PO, RIGHT),
        Specifier::new("m_share",     "SHRD",        Printer::Nop,          Item::MemShrPgs,      5,  PO, RIGHT),
        Specifier::new("m_size",      "SIZE",        Printer::Size,         Item::VsizeBytes,     5,  PO, RIGHT),
        Specifier::new("m_swap",      "SWAP",        Printer::Nop,          Item::Noop,           5,  PO, RIGHT),
        Specifier::new("m_trs",       "TRS",         Printer::Trs,          Item::VsizeBytes,     5,  PO, RIGHT),
        Specifier::new("machine",     "MACHINE",     Printer::SdMachine,    Item::SdMach,         31, ET, LEFT),
        Specifier::new("maj_flt",     "MAJFL",       Printer::Majflt,       Item::FltMaj,         6,  AN, RIGHT),
        Specifier::new("majflt",      "MAJFLT",      Printer::Majflt,       Item::FltMaj,         6,  AN, RIGHT),
        Specifier::new("min_flt",     "MINFL",       Printer::Minflt,       Item::FltMin,         6,  AN, RIGHT),
        Specifier::new("minflt",      "MINFLT",      Printer::Minflt,       Item::FltMin,         6,  AN, RIGHT),
        Specifier::new("mntns",       "MNTNS",       Printer::Mntns,        Item::NsMnt,          10, ET, RIGHT),
        Specifier::new("msgrcv",      "MSGRCV",      Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("msgsnd",      "MSGSND",      Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("mwchan",      "MWCHAN",      Printer::Nop,          Item::Noop,           6,  TO, WCHAN),
        Specifier::new("netns",       "NETNS",       Printer::Netns,        Item::NsNet,          10, ET, RIGHT),
        Specifier::new("ni",          "NI",          Printer::Nice,         Item::Nice,           3,  TO, RIGHT),
        Specifier::new("nice",        "NI",          Printer::Nice,         Item::Nice,           3,  TO, RIGHT),
        Specifier::new("nivcsw",      "IVCSW",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("nlwp",        "NLWP",        Printer::Nlwp,         Item::Nlwp,           4,  PO, RIGHT),
        Specifier::new("nsignals",    "NSIGS",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("nsigs",       "NSIGS",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("nswap",       "NSWAP",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("numa",        "NUMA",        Printer::Numa,         Item::ProcessorNode,  4,  AN, RIGHT),
        Specifier::new("nvcsw",       "VCSW",        Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("nwchan",      "WCHAN",       Printer::Nop,          Item::Noop,           6,  TO, RIGHT),
        Specifier::new("oom",         "OOM",         Printer::Oom,          Item::OomScore,       4,  TO, RIGHT),
        Specifier::new("oomadj",      "OOMADJ",      Printer::OomAdj,       Item::OomAdj,         5,  TO, RIGHT),
        Specifier::new("opri",        "PRI",         Printer::Opri,         Item::Priority,       3,  TO, RIGHT),
        Specifier::new("osz",         "SZ",          Printer::Nop,          Item::Noop,           2,  PO, RIGHT),
        Specifier::new("oublk",       "OUBLK",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("oublock",     "OUBLK",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("ouid",        "OWNER",       Printer::SdOuid,       Item::SdOuid,         5,  ET, LEFT),
        Specifier::new("p_ru",        "P_RU",        Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("paddr",       "PADDR",       Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("pagein",      "PAGEIN",      Printer::Majflt,       Item::FltMaj,         6,  AN, RIGHT),
        Specifier::new("pcap",        "PCAP",        Printer::Pcap,         Item::CapsPermitted,  16, TO, RIGHT),
        Specifier::new("pcaps",       "PCAPS",       Printer::Pcaps,        Item::CapsPermitted,  16, TO, RIGHT),
        Specifier::new("pcpu",        "%CPU",        Printer::Pcpu,         Item::Utilization,    4,  ET, RIGHT),
        Specifier::new("pending",     "PENDING",     Printer::Sig,          Item::Signals,        9,  ET, SIGNAL),
        Specifier::new("pgid",        "PGID",        Printer::Pgid,         Item::IdPgrp,         5,  PO, RIGHT).with_pid_width(),
        Specifier::new("pgrp",        "PGRP",        Printer::Pgid,         Item::IdPgrp,         5,  PO, RIGHT).with_pid_width(),
        Specifier::new("pid",         "PID",         Printer::Procs,        Item::IdTgid,         5,  PO, RIGHT).with_pid_width(),
        Specifier::new("pidns",       "PIDNS",       Printer::Pidns,        Item::NsPid,          10, ET, RIGHT),
        Specifier::new("pmem",        "%MEM",        Printer::Pmem,         Item::VmRss,          4,  PO, RIGHT),
        Specifier::new("poip",        "-",           Printer::Nop,          Item::Noop,           1,  AN, RIGHT),
        Specifier::new("policy",      "POL",         Printer::Class,        Item::SchedClassstr,  3,  TO, LEFT),
        Specifier::new("ppid",        "PPID",        Printer::Ppid,         Item::IdPpid,         5,  PO, RIGHT).with_pid_width(),
        Specifier::new("pri",         "PRI",         Printer::Pri,          Item::Priority,       3,  TO, RIGHT),
        Specifier::new("pri_api",     "API",         Printer::PriApi,       Item::Priority,       3,  TO, RIGHT),
        Specifier::new("pri_bar",     "BAR",         Printer::PriBar,       Item::Priority,       3,  TO, RIGHT),
        Specifier::new("pri_baz",     "BAZ",         Printer::PriBaz,       Item::Priority,       3,  TO, RIGHT),
        Specifier::new("pri_foo",     "FOO",         Printer::PriFoo,       Item::Priority,       3,  TO, RIGHT),
        Specifier::new("priority",    "PRI",         Printer::Priority,     Item::Priority,       3,  TO, RIGHT),
        Specifier::new("prmgrp",      "PRMGRP",      Printer::Nop,          Item::Noop,           12, PO, RIGHT),
        Specifier::new("prmid",       "PRMID",       Printer::Nop,          Item::Noop,           12, PO, RIGHT),
        Specifier::new("project",     "PROJECT",     Printer::Nop,          Item::Noop,           12, PO, LEFT),
        Specifier::new("projid",      "PROJID",      Printer::Nop,          Item::Noop,           5,  PO, RIGHT),
        Specifier::new("pset",        "PSET",        Printer::Nop,          Item::Noop,           4,  TO, RIGHT),
        Specifier::new("psr",         "PSR",         Printer::Psr,          Item::Processor,      3,  TO, RIGHT),
        Specifier::new("pss",         "PSS",         Printer::Pss,          Item::SmapPss,        5,  PO, RIGHT),
        Specifier::new("psxpri",      "PPR",         Printer::Nop,          Item::Noop,           3,  TO, RIGHT),
        Specifier::new("rbytes",      "RBYTES",      Printer::Rbytes,       Item::IoReadBytes,    5,  TO, RIGHT),
        Specifier::new("rchars",      "RCHARS",      Printer::Rchars,       Item::IoReadChars,    5,  TO, RIGHT),
        Specifier::new("re",          "RE",          Printer::Nop,          Item::Noop,           3,  AN, RIGHT),
        Specifier::new("resident",    "RES",         Printer::Nop,          Item::MemResPgs,      5,  PO, RIGHT),
        Specifier::new("rgid",        "RGID",        Printer::Rgid,         Item::IdRgid,         5,  ET, RIGHT),
        Specifier::new("rgroup",      "RGROUP",      Printer::Rgroup,       Item::IdRgroup,       8,  ET, USER),
        Specifier::new("rlink",       "RLINK",       Printer::Nop,          Item::Noop,           8,  AN, RIGHT),
        Specifier::new("rops",        "ROPS",        Printer::Rops,         Item::IoReadOps,      5,  TO, RIGHT),
        Specifier::new("rss",         "RSS",         Printer::Rss,          Item::VmRss,          5,  PO, RIGHT),
        Specifier::new("rssize",      "RSS",         Printer::Rss,          Item::VmRss,          5,  PO, RIGHT),
        Specifier::new("rsz",         "RSZ",         Printer::Rss,          Item::VmRss,          5,  PO, RIGHT),
        Specifier::new("rtprio",      "RTPRIO",      Printer::Rtprio,       Item::PriorityRt,     6,  TO, RIGHT),
        Specifier::new("ruid",        "RUID",        Printer::Ruid,         Item::IdRuid,         5,  ET, RIGHT),
        Specifier::new("ruser",       "RUSER",       Printer::Ruser,        Item::IdRuser,        8,  ET, USER),
        Specifier::new("s",           "S",           Printer::S,            Item::State,          1,  TO, LEFT),
        Specifier::new("sched",       "SCH",         Printer::Sched,        Item::SchedClass,     3,  TO, RIGHT),
        Specifier::new("scnt",        "SCNT",        Printer::Nop,          Item::Noop,           4,  AN, RIGHT),
        Specifier::new("scount",      "SC",          Printer::Nop,          Item::Noop,           4,  AN, RIGHT),
        Specifier::new("seat",        "SEAT",        Printer::SdSeat,       Item::SdSeat,         11, ET, LEFT),
        Specifier::new("sess",        "SESS",        Printer::Sess,         Item::IdSession,      5,  PO, RIGHT).with_pid_width(),
        Specifier::new("session",     "SESS",        Printer::Sess,         Item::IdSession,      5,  PO, RIGHT).with_pid_width(),
        Specifier::new("sgi_p",       "P",           Printer::SgiP,         Item::State,          1,  TO, RIGHT),
        Specifier::new("sgi_rss",     "RSS",         Printer::Rss,          Item::VmRss,          4,  PO, LEFT),
        Specifier::new("sgid",        "SGID",        Printer::Sgid,         Item::IdSgid,         5,  ET, RIGHT),
        Specifier::new("sgroup",      "SGROUP",      Printer::Sgroup,       Item::IdSgroup,       8,  ET, USER),
        Specifier::new("share",       "-",           Printer::Nop,          Item::Noop,           1,  PO, RIGHT),
        Specifier::new("sid",         "SID",         Printer::Sess,         Item::IdSession,      5,  PO, RIGHT).with_pid_width(),
        Specifier::new("sig",         "PENDING",     Printer::Sig,          Item::Signals,        9,  ET, SIGNAL),
        Specifier::new("sig_block",   "BLOCKED",     Printer::Sigmask,      Item::Sigblocked,     9,  TO, SIGNAL),
        Specifier::new("sig_catch",   "CATCHED",     Printer::Sigcatch,     Item::Sigcatch,       9,  TO, SIGNAL),
        Specifier::new("sig_ignore",  "IGNORED",     Printer::Sigignore,    Item::Sigignore,      9,  TO, SIGNAL),
        Specifier::new("sig_pend",    "SIGNAL",      Printer::Sig,          Item::Signals,        9,  ET, SIGNAL),
        Specifier::new("sigcatch",    "CAUGHT",      Printer::Sigcatch,     Item::Sigcatch,       9,  TO, SIGNAL),
        Specifier::new("sigignore",   "IGNORED",     Printer::Sigignore,    Item::Sigignore,      9,  TO, SIGNAL),
        Specifier::new("sigmask",     "BLOCKED",     Printer::Sigmask,      Item::Sigblocked,     9,  TO, SIGNAL),
        Specifier::new("size",        "SIZE",        Printer::Swapable,     Item::VsizeBytes,     5,  PO, RIGHT),
        Specifier::new("sl",          "SL",          Printer::Nop,          Item::Noop,           3,  AN, RIGHT),
        Specifier::new("slice",       "SLICE",       Printer::SdSlice,      Item::SdSlice,        31, ET, LEFT),
        Specifier::new("spid",        "SPID",        Printer::Tasks,        Item::IdPid,          5,  TO, RIGHT).with_pid_width(),
        Specifier::new("stackp",      "STACKP",      Printer::Stackp,       Item::AddrStackStart, 16, PO, RIGHT),
        Specifier::new("start",       "STARTED",     Printer::Start,        Item::TicsBegan,      8,  ET, RIGHT),
        Specifier::new("start_code",  "S_CODE",      Printer::Nop,          Item::AddrCodeStart,  16, PO, RIGHT),
        Specifier::new("start_stack", "STACKP",      Printer::Stackp,       Item::AddrStackStart, 16, PO, RIGHT),
        Specifier::new("start_time",  "START",       Printer::Stime,        Item::TicsBegan,      5,  ET, RIGHT),
        Specifier::new("stat",        "STAT",        Printer::Stat,         Item::State,          4,  TO, LEFT),
        Specifier::new("state",       "S",           Printer::S,            Item::State,          1,  TO, LEFT),
        Specifier::new("status",      "STATUS",      Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("stime",       "STIME",       Printer::Stime,        Item::TicsBegan,      5,  ET, RIGHT),
        Specifier::new("suid",        "SUID",        Printer::Suid,         Item::IdSuid,         5,  ET, RIGHT),
        Specifier::new("supgid",      "SUPGID",      Printer::Supgid,       Item::Supgids,        20, PO, UNLIMITED),
        Specifier::new("supgrp",      "SUPGRP",      Printer::Supgrp,       Item::Supgroups,      40, PO, UNLIMITED),
        Specifier::new("suser",       "SUSER",       Printer::Suser,        Item::IdSuser,        8,  ET, USER),
        Specifier::new("svgid",       "SVGID",       Printer::Sgid,         Item::IdSgid,         5,  ET, RIGHT),
        Specifier::new("svgroup",     "SVGROUP",     Printer::Sgroup,       Item::IdSgroup,       8,  ET, USER),
        Specifier::new("svuid",       "SVUID",       Printer::Suid,         Item::IdSuid,         5,  ET, RIGHT),
        Specifier::new("svuser",      "SVUSER",      Printer::Suser,        Item::IdSuser,        8,  ET, USER),
        Specifier::new("systime",     "SYSTEM",      Printer::Nop,          Item::Noop,           6,  ET, RIGHT),
        Specifier::new("sz",          "SZ",          Printer::Sz,           Item::VmSize,         5,  PO, RIGHT),
        Specifier::new("taskid",      "TASKID",      Printer::Nop,          Item::Noop,           5,  TO, RIGHT).with_pid_width(),
        Specifier::new("tdev",        "TDEV",        Printer::Nop,          Item::Noop,           4,  AN, RIGHT),
        Specifier::new("tgid",        "TGID",        Printer::Procs,        Item::IdTgid,         5,  PO, RIGHT).with_pid_width(),
        Specifier::new("thcount",     "THCNT",       Printer::Nlwp,         Item::Nlwp,           5,  PO, RIGHT),
        Specifier::new("tid",         "TID",         Printer::Tasks,        Item::IdPid,          5,  TO, RIGHT).with_pid_width(),
        Specifier::new("time",        "TIME",        Printer::Time,         Item::TimeAll,        8,  ET, RIGHT),
        Specifier::new("timens",      "TIMENS",      Printer::Timens,       Item::NsTime,         10, ET, RIGHT),
        Specifier::new("timeout",     "TMOUT",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("times",       "TIME",        Printer::Times,        Item::TimeAll,        8,  ET, RIGHT),
        Specifier::new("tmout",       "TMOUT",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("tname",       "TTY",         Printer::Tty8,         Item::TtyName,        8,  PO, LEFT),
        Specifier::new("tpgid",       "TPGID",       Printer::Tpgid,        Item::IdTpgid,        5,  PO, RIGHT).with_pid_width(),
        Specifier::new("trs",         "TRS",         Printer::Trs,          Item::VsizeBytes,     4,  PO, RIGHT),
        Specifier::new("trss",        "TRSS",        Printer::Trs,          Item::VsizeBytes,     4,  PO, RIGHT),
        Specifier::new("tsess",       "TSESS",       Printer::Nop,          Item::Noop,           5,  PO, RIGHT).with_pid_width(),
        Specifier::new("tsession",    "TSESS",       Printer::Nop,          Item::Noop,           5,  PO, RIGHT).with_pid_width(),
        Specifier::new("tsid",        "TSID",        Printer::Nop,          Item::Noop,           5,  PO, RIGHT).with_pid_width(),
        Specifier::new("tsig",        "PENDING",     Printer::Tsig,         Item::Sigpending,     9,  ET, SIGNAL),
        Specifier::new("tsiz",        "TSIZ",        Printer::Tsiz,         Item::VsizeBytes,     4,  PO, RIGHT),
        Specifier::new("tt",          "TT",          Printer::Tty8,         Item::TtyName,        8,  PO, LEFT),
        Specifier::new("tty",         "TT",          Printer::Tty8,         Item::TtyName,        8,  PO, LEFT),
        Specifier::new("tty4",        "TTY",         Printer::Tty4,         Item::TtyName,        4,  PO, LEFT),
        Specifier::new("tty8",        "TTY",         Printer::Tty8,         Item::TtyName,        8,  PO, LEFT),
        Specifier::new("u_procp",     "UPROCP",      Printer::Nop,          Item::Noop,           6,  AN, RIGHT),
        Specifier::new("ucmd",        "CMD",         Printer::Comm,         Item::Cmd,            15, PO, UNLIMITED),
        Specifier::new("ucomm",       "COMMAND",     Printer::Comm,         Item::Cmd,            15, PO, UNLIMITED),
        Specifier::new("uid",         "UID",         Printer::Euid,         Item::IdEuid,         5,  ET, RIGHT),
        Specifier::new("uid_hack",    "UID",         Printer::Euser,        Item::IdEuser,        8,  ET, USER),
        Specifier::new("umask",       "UMASK",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("uname",       "USER",        Printer::Euser,        Item::IdEuser,        8,  ET, USER),
        Specifier::new("unit",        "UNIT",        Printer::SdUnit,       Item::SdUnit,         31, ET, LEFT),
        Specifier::new("upr",         "UPR",         Printer::Nop,          Item::Noop,           3,  TO, RIGHT),
        Specifier::new("uprocp",      "UPROCP",      Printer::Nop,          Item::Noop,           8,  AN, RIGHT),
        Specifier::new("user",        "USER",        Printer::Euser,        Item::IdEuser,        8,  ET, USER),
        Specifier::new("userns",      "USERNS",      Printer::Userns,       Item::NsUser,         10, ET, RIGHT),
        Specifier::new("usertime",    "USER",        Printer::Nop,          Item::Noop,           4,  ET, RIGHT),
        Specifier::new("usrpri",      "UPR",         Printer::Nop,          Item::Noop,           3,  TO, RIGHT),
        Specifier::new("uss",         "USS",         Printer::Uss,          Item::SmapPrvTotal,   5,  PO, RIGHT),
        Specifier::new("util",        "C",           Printer::C,            Item::Utilization,    2,  ET, RIGHT),
        Specifier::new("utime",       "UTIME",       Printer::Nop,          Item::TicsUser,       6,  ET, RIGHT),
        Specifier::new("utsns",       "UTSNS",       Printer::Utsns,        Item::NsUts,          10, ET, RIGHT),
        Specifier::new("uunit",       "UUNIT",       Printer::SdUunit,      Item::SdUunit,        31, ET, LEFT),
        Specifier::new("vm_data",     "DATA",        Printer::Nop,          Item::VmData,         5,  PO, RIGHT),
        Specifier::new("vm_exe",      "EXE",         Printer::Nop,          Item::VmExe,          5,  PO, RIGHT),
        Specifier::new("vm_lib",      "LIB",         Printer::Nop,          Item::VmLib,          5,  PO, RIGHT),
        Specifier::new("vm_lock",     "LCK",         Printer::Nop,          Item::VmRssLocked,    3,  PO, RIGHT),
        Specifier::new("vm_stack",    "STACK",       Printer::Nop,          Item::VmStack,        5,  PO, RIGHT),
        Specifier::new("vsize",       "VSZ",         Printer::Vsz,          Item::VsizeBytes,     6,  PO, RIGHT),
        Specifier::new("vsz",         "VSZ",         Printer::Vsz,          Item::VmSize,         6,  PO, RIGHT),
        Specifier::new("wbytes",      "WBYTES",      Printer::Wbytes,       Item::IoWriteBytes,   5,  TO, RIGHT),
        Specifier::new("wcbytes",     "WCBYTES",     Printer::Wcbytes,      Item::IoWriteCbytes,  5,  TO, RIGHT),
        Specifier::new("wchan",       "WCHAN",       Printer::Wchan,        Item::WchanName,      6,  TO, WCHAN),
        Specifier::new("wchars",      "WCHARS",      Printer::Wchars,       Item::IoWriteChars,   5,  TO, RIGHT),
        Specifier::new("wname",       "WCHAN",       Printer::Wchan,        Item::WchanName,      6,  TO, WCHAN),
        Specifier::new("wops",        "WOPS",        Printer::Wops,         Item::IoWriteOps,     5,  TO, RIGHT),
        Specifier::new("xstat",       "XSTAT",       Printer::Nop,          Item::Noop,           5,  AN, RIGHT),
        Specifier::new("zone",        "ZONE",        Printer::Context,      Item::IdTgid,         31, ET, LEFT),
        Specifier::new("zoneid",      "ZONEID",      Printer::Nop,          Item::Noop,           31, ET, RIGHT),
        Specifier::new("~",           "-",           Printer::Nop,          Item::Noop,           1,  AN, RIGHT),
    ]
};

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
        .filter(|specifier| !specifier.is_placeholder())
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
        let found = find(name.as_bytes()).map(|specifier| {
            (
                specifier.header,
                specifier.width(),
                specifier.is_placeholder(),
            )
        });

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
