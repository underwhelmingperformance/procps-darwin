// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 1998-2002 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/common.h and src/ps/global.c.

use crate::ps::{
    format::{Column, SortKey},
    help::Section,
    personality::Personality,
    selection::Selection,
};

bitflags::bitflags! {
    /// How `ps` lists threads (`TF_*`).
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct ThreadFlags: u32 {
        /// BSD's `H`.
        const B_H = 0x0001;
        /// BSD's `m`.
        const B_M = 0x0002;
        /// UNIX's `-m`.
        const U_M = 0x0004;
        /// UNIX's `-T`.
        const U_T = 0x0008;
        /// UNIX's `-L`.
        const U_L = 0x0010;
        /// Show a row for each process.
        const SHOW_PROCESS = 0x0100;
        /// Show a row for each thread.
        const SHOW_TASK = 0x0200;
        /// Show rows for processes and their threads, with different columns.
        const SHOW_BOTH = 0x0400;
        /// Let sorting separate a process's threads.
        const LOOSE_TASKS = 0x0800;
        /// The thread options make sense only with an `LWP` or `SPID` column.
        const MUST_USE = 0x4000;
    }
}

bitflags::bitflags! {
    /// The predefined formats that the options select (`FF_*`).
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct FormatFlags: u32 {
        /// `-f`.
        const U_F = 0x0001;
        /// `-j`, when the personality defines a format for it.
        const U_J = 0x0002;
        /// `-l`.
        const U_L = 0x0004;
        /// BSD's `j`.
        const B_J = 0x0008;
        /// BSD's `l`.
        const B_L = 0x0010;
        /// BSD's `s`.
        const B_S = 0x0020;
        /// BSD's `u`.
        const B_U = 0x0040;
        /// BSD's `v`.
        const B_V = 0x0080;
        /// `X`.
        const L_X = 0x0100;
        /// `m`, with the `old` personality.
        const L_M = 0x0200;
        /// `--context`.
        const F_C = 0x0400;
    }
}

bitflags::bitflags! {
    /// The options that change a predefined format (`FM_*`).
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct FormatModifiers: u32 {
        /// `-c`.
        const C = 0x0001;
        /// `-j`, when the personality has no format for it.
        const J = 0x0002;
        /// `-y`, and `-x` with the `svr4` personality.
        const Y = 0x0004;
        /// `-P`.
        const P = 0x0010;
        /// `-M`, `-Z` and BSD's `Z`.
        const M = 0x0020;
        /// `-F`.
        const F = 0x0080;
    }
}

bitflags::bitflags! {
    /// The options that change how columns show their values.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct Modifiers: u32 {
        /// BSD's `c`: the `args`, `cmd` and `command` columns show the
        /// command name in place of the command line.
        const COMMAND_NAMES = 0x01;
        /// BSD's `e`: show the environment after the command line.
        const ENVIRONMENT = 0x02;
        /// BSD's `S` and `--cumulative`: add the CPU time of dead children.
        const CUMULATIVE = 0x04;
        /// BSD's `n`: show user IDs in place of user names, and the `WCHAN`
        /// column as a number.
        const NUMERIC = 0x08;
        /// UNIX's `-f` and `-F`: the `comm`, `ucmd` and `ucomm` columns show
        /// the command line in place of the command name.
        const FULL = 0x10;
        /// `--signames`: show signal masks as signal names.
        const SIGNAL_NAMES = 0x20;
    }
}

/// Where the header line is printed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Header {
    /// Once, at the top (`HEAD_SINGLE`).
    #[default]
    Single,
    /// Never (`HEAD_NONE`).
    None,
    /// At the top of each screen (`HEAD_MULTI`).
    Multiple,
}

/// The process tree that the options ask for (`forest_type`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Forest {
    /// UNIX's `-H`, which indents each child.
    Unix,
    /// BSD's `f`.
    Bsd,
    /// `--forest`.
    Gnu,
}

/// The screen size that `ps` lays its output out for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Screen {
    /// The number of columns.
    pub(crate) columns: i64,
    /// The number of rows.
    pub(crate) rows: i64,
}

/// What `--info` prints, apart from the values that `ps` reads from the
/// kernel when it prints them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Info {
    /// The personality.
    pub(crate) personality: Personality,
    /// The screen size.
    pub(crate) screen: Screen,
}

/// The options of a `ps` command line that lists processes, after
/// procps-ng's checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Options {
    /// The processes to list.
    pub(crate) selection: Selection,
    /// The columns, in order.
    pub(crate) columns: Vec<Column>,
    /// The sort keys, in the order that procps-ng applies them. Each key
    /// sorts the whole list again, so the last key is the most significant.
    pub(crate) sort: Vec<SortKey>,
    /// How `ps` lists threads.
    pub(crate) threads: ThreadFlags,
    /// The process tree, if one is shown.
    pub(crate) forest: Option<Forest>,
    /// Where the header line is printed.
    pub(crate) header: Header,
    /// The options that change how columns show their values.
    pub(crate) modifiers: Modifiers,
    /// `-D` and `--date-format`: the format of the `lstart` column.
    pub(crate) date_format: Option<Vec<u8>>,
    /// `--delimiter`: the byte between columns.
    pub(crate) delimiter: Option<u8>,
    /// The screen size.
    pub(crate) screen: Screen,
}

/// What a `ps` command line asks `ps` to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    /// Print a section of the usage text on standard output and exit with
    /// status 0 (`--help`).
    Help(Section),
    /// Print the version and exit with status 0 (`-V`, `V` and
    /// `--version`).
    Version,
    /// Print the format specifiers and exit with status 0 (BSD's `L`).
    ListSpecifiers,
    /// Print debugging information on standard error and exit with status 0
    /// (`--info`).
    Info(Box<Info>),
    /// List processes as the [`Options`] describe.
    Run(Box<Options>),
}
