// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2012-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2012-2014 Jaromir Capik <jcapik@redhat.com>
// SPDX-FileCopyrightText: 1998-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/help.c.

use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

/// A section of `ps`'s usage text, which `--help` selects by its word or its
/// abbreviation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Section {
    /// `simple` or `s`: the basic selection options.
    Simple,
    /// `list` or `l`: selection by list.
    List,
    /// `output` or `o`: the output formats.
    Output,
    /// `threads` or `t`: the thread displays.
    Threads,
    /// `misc` or `m`: the other options.
    Miscellaneous,
    /// `all` or `a`: every section.
    All,
    /// No section: a line that lists the sections.
    Summary,
}

/// The sections that `--help` selects, with their words and abbreviations.
const SECTIONS: [(Section, &str, &str); 6] = [
    (Section::Simple, "simple", "s"),
    (Section::List, "list", "l"),
    (Section::Output, "output", "o"),
    (Section::Threads, "threads", "t"),
    (Section::Miscellaneous, "misc", "m"),
    (Section::All, "all", "a"),
];

const SIMPLE: &str = "
Basic options:
 -A, -e               all processes
 -a                   all with tty, except session leaders
  a                   all with tty, including other users
 -d                   all except session leaders
 -N, --deselect       negate selection
  r                   only running processes
  T                   all processes on this terminal
  x                   processes without controlling ttys
";

const LIST: &str = "
Selection by list:
 -C <command>         command name
 -G, --Group <GID>    real group id or name
 -g, --group <group>  session or effective group name
 -p, p, --pid <PID>   process id
        --ppid <PID>  parent process id
 -q, q, --quick-pid <PID>
                      process id (quick mode)
 -s, --sid <session>  session id
 -t, t, --tty <tty>   terminal
 -u, U, --user <UID>  effective user id or name
 -U, --User <UID>     real user id or name

  The selection options take as their argument either:
    a comma-separated list e.g. '-u root,nobody' or
    a blank-separated list e.g. '-p 123 4567'
";

const OUTPUT: &str = "
Output formats:
 -D <format>          date format for lstart
 -F                   extra full
 -f                   full-format, including command lines
  f, --forest         ascii art process tree
 -H                   show process hierarchy
 -j                   jobs format
  j                   BSD job control format
 -l                   long format
  l                   BSD long format
 -M, Z                add security data (for SELinux)
 -O <format>          preloaded with default columns
  O <format>          as -O, with BSD personality
 -o, o, --format <format>
                      user-defined format
  -P                  add psr column
  s                   signal format
  u                   user-oriented format
  v                   virtual memory format
  X                   register format
 -y                   do not show flags, show rss vs. addr (used with -l)
     --context        display security context (for SELinux)
     --delimiter <d>  Use <d> as a column delimiter instead of variable space
     --headers        repeat header lines, one per page
     --no-headers     do not print header at all
     --cols, --columns, --width <num>
                      set screen width
     --rows, --lines <num>
                      set screen height
     --signames       display signal masks using signal names
";

const THREADS: &str = "
Show threads:
  H                   as if they were processes
 -L                   possibly with LWP and NLWP columns
 -m, m                after processes
 -T                   possibly with SPID column
";

const MISCELLANEOUS: &str = "
Miscellaneous options:
 -c                   show scheduling class with -l option
  c                   show true command name
  e                   show the environment after command
  k,    --sort        specify sort order as: [+|-]key[,[+|-]key[,...]]
  L                   show format specifiers
  n                   display numeric uid and wchan
  S,    --cumulative  include some dead child process data
 -y                   do not show flags, show rss (only with -l)
 -V, V, --version     display version information and exit
 -w, w                unlimited output width

        --help <simple|list|output|threads|misc|all>
                      display help and exit
";

impl Section {
    /// The section that `--help` selects with `word`, or [`Section::Summary`]
    /// for no word or an unknown one, as procps-ng's `parse_help_opt` decides.
    pub(crate) fn from_word(word: Option<&[u8]>) -> Self {
        SECTIONS
            .iter()
            .find(|(_, name, abbreviation)| {
                word == Some(name.as_bytes()) || word == Some(abbreviation.as_bytes())
            })
            .map_or(Self::Summary, |&(section, ..)| section)
    }

    /// Whether this section includes `part`.
    fn includes(self, part: Self) -> bool {
        self == part || (self == Self::All && part != Self::Summary)
    }
}

/// The usage text that procps-ng's `do_help` prints for `section`. `program`
/// is the name that `ps` was run as, without its directory.
pub(crate) fn usage(program: &OsStr, section: Section) -> Vec<u8> {
    let parts = [
        (Section::Simple, SIMPLE),
        (Section::List, LIST),
        (Section::Output, OUTPUT),
        (Section::Threads, THREADS),
        (Section::Miscellaneous, MISCELLANEOUS),
    ];
    let mut text = [b"\nUsage:\n ", program.as_bytes(), b" [options]\n"].concat();

    for (part, part_text) in parts {
        if section.includes(part) {
            text.extend_from_slice(part_text.as_bytes());
        }
    }

    if section == Section::Summary {
        let program = program.as_bytes();

        text.extend_from_slice(
            &[
                b"\n Try '",
                program,
                b" --help <simple|list|output|threads|misc|all>'\n  or '",
                program,
                b" --help <s|l|o|t|m|a>'\n for additional help text.\n",
            ]
            .concat(),
        );
    }

    text.extend_from_slice(b"\nFor more details see ps(1).\n");
    text
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{LIST, MISCELLANEOUS, OUTPUT, SIMPLE, Section, THREADS, usage};

    #[rstest]
    #[case::word(Some(&b"threads"[..]), Section::Threads)]
    #[case::abbreviation(Some(&b"m"[..]), Section::Miscellaneous)]
    #[case::all(Some(&b"a"[..]), Section::All)]
    #[case::unknown(Some(&b"thread"[..]), Section::Summary)]
    #[case::none(None, Section::Summary)]
    fn a_word_selects_a_section(#[case] word: Option<&[u8]>, #[case] expected: Section) {
        assert_eq!(Section::from_word(word), expected);
    }

    #[rstest]
    #[case::summary(
        Section::Summary,
        "\nUsage:\n /bin/ps [options]\n\n Try '/bin/ps --help <simple|list|output|threads|misc|all>'\n  or '/bin/ps --help <s|l|o|t|m|a>'\n for additional help text.\n\nFor more details see ps(1).\n"
    )]
    #[case::threads(
        Section::Threads,
        "\nUsage:\n /bin/ps [options]\n\nShow threads:\n  H                   as if they were processes\n -L                   possibly with LWP and NLWP columns\n -m, m                after processes\n -T                   possibly with SPID column\n\nFor more details see ps(1).\n"
    )]
    fn the_usage_text_has_the_section(#[case] section: Section, #[case] expected: &str) {
        assert_eq!(
            String::from_utf8(usage("/bin/ps".as_ref(), section)),
            Ok(expected.to_owned())
        );
    }

    #[test]
    fn all_has_every_section_in_order() {
        let expected = [
            "\nUsage:\n ps [options]\n",
            SIMPLE,
            LIST,
            OUTPUT,
            THREADS,
            MISCELLANEOUS,
            "\nFor more details see ps(1).\n",
        ]
        .concat();

        assert_eq!(
            String::from_utf8(usage("ps".as_ref(), Section::All)),
            Ok(expected)
        );
    }
}
