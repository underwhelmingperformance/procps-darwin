// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 1998-2002 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/global.c.

use std::os::unix::ffi::OsStrExt;

use crate::ps::system::System;

bitflags::bitflags! {
    /// How a personality changes the parser and the predefined formats
    /// (`PER_*`).
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(crate) struct Flags: u32 {
        /// BSD's `h` repeats the header, in place of leaving it out.
        const BSD_H = 0x0002;
        /// BSD's `m` sorts by memory.
        const BSD_M = 0x0004;
        /// `-l` has IRIX's columns.
        const IRIX_L = 0x0008;
        /// Skip the first pass of the parser, so that an argument that is a
        /// dash followed by a letter is read as BSD options.
        const FORCE_BSD = 0x0010;
        /// UNIX's `-o` reads its format as other personalities do.
        const GOOD_O = 0x0020;
        /// BSD's `m` selects old Linux's memory format.
        const OLD_M = 0x0040;
        /// Session leaders are selected only with BSD's `g`.
        const NO_DEFAULT_G = 0x0080;
        /// `-l` has a `P` column in place of `ADDR`.
        const ZAP_ADDR = 0x0100;
        /// `-f` has a `USER` column in place of `UID`.
        const SANE_USER = 0x0200;
        /// `-x` widens the command line, as on HP-UX.
        const HPUX_X = 0x0400;
        /// `-x` is `-y`, as on System V Release 4.
        const SVR4_X = 0x0800;
    }
}

/// The macros for BSD's predefined formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BsdFormats {
    /// `j`.
    pub(crate) j: &'static str,
    /// `l`.
    pub(crate) l: &'static str,
    /// `s`.
    pub(crate) s: &'static str,
    /// `u`.
    pub(crate) u: &'static str,
    /// `v`.
    pub(crate) v: &'static str,
}

/// The macros for UNIX's predefined formats. A format without a macro has
/// columns that procps-ng chooses from the options.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UnixFormats {
    /// `-f`.
    pub(crate) f: Option<&'static str>,
    /// `-f` with `-l`.
    pub(crate) fl: Option<&'static str>,
    /// `-j`.
    pub(crate) j: Option<&'static str>,
    /// `-l`.
    pub(crate) l: Option<&'static str>,
}

/// The behaviour that the `PS_PERSONALITY` environment variable selects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Personality {
    /// How the personality changes the parser and the predefined formats.
    pub(crate) flags: Flags,
    /// Whether the default selection and format are BSD's.
    pub(crate) prefer_bsd_defaults: bool,
    /// The macros for BSD's predefined formats.
    pub(crate) bsd: BsdFormats,
    /// The macros for UNIX's predefined formats.
    pub(crate) unix: UnixFormats,
    /// The name that `--info` shows: the personality's name as the
    /// environment gives it, or procps-ng's text for a name that is too long.
    pub(crate) name: Vec<u8>,
}

/// The macros of the old Linux formats, which most personalities use.
const OLD_LINUX: BsdFormats = BsdFormats {
    j: "OL_j",
    l: "OL_l",
    s: "OL_s",
    u: "OL_u",
    v: "OL_v",
};

/// The macros of FreeBSD's formats. FreeBSD has no `s` format, so `s` keeps
/// old Linux's.
const FREEBSD: BsdFormats = BsdFormats {
    j: "FB_j",
    l: "FB_l",
    s: "OL_s",
    u: "FB_u",
    v: "FB_v",
};

/// The longest personality name that procps-ng reads. Its buffer has 16
/// bytes, and the last is the NUL.
const NAME_MAX: usize = 15;

/// The personality names, in procps-ng's table, which `strcasecmp` compares.
const NAMES: [&str; 27] = [
    "390", "aix", "bsd", "compaq", "debian", "default", "digital", "gnu", "hp", "hpux", "irix",
    "linux", "old", "os390", "posix", "s390", "sco", "sgi", "solaris2", "sunos4", "svr4", "sysv",
    "tru64", "unix", "unix95", "unix98", "unknown",
];

impl Personality {
    /// The personality without any of procps-ng's changes, whose name
    /// `--info` shows as `name`.
    fn defaults(name: Vec<u8>) -> Self {
        Self {
            flags: Flags::empty(),
            prefer_bsd_defaults: false,
            bsd: OLD_LINUX,
            unix: UnixFormats::default(),
            name,
        }
    }

    /// The personality that the environment selects, as procps-ng's
    /// `set_personality` reads it: from `PS_PERSONALITY`, then `CMD_ENV`,
    /// and `old` whenever `I_WANT_A_BROKEN_PS` is set. An unknown name
    /// selects the defaults.
    pub(crate) fn from_environment(system: &impl System) -> Self {
        let non_empty = |name| system.variable(name).filter(|value| !value.is_empty());
        let mut name = non_empty("PS_PERSONALITY")
            .or_else(|| non_empty("CMD_ENV"))
            .map_or_else(|| b"unknown".to_vec(), |value| value.as_bytes().to_vec());

        if system.variable("I_WANT_A_BROKEN_PS").is_some() {
            name = b"old".to_vec();
        }

        if name.len() > NAME_MAX {
            return Self::defaults(b"You found a bug!".to_vec());
        }

        let folding = system.case_folding();
        let known = NAMES
            .iter()
            .find(|known| folding.eq_ignore_case(&name, known.as_bytes()));
        let mut personality = Self::defaults(name);

        let Some(&known) = known else {
            return personality;
        };

        match known {
            "bsd" => {
                personality.flags = Flags::FORCE_BSD | Flags::BSD_H | Flags::BSD_M;
                personality.prefer_bsd_defaults = true;
                personality.bsd = FREEBSD;
            }
            "old" => {
                personality.flags = Flags::FORCE_BSD | Flags::OLD_M;
                personality.prefer_bsd_defaults = true;
            }
            "debian" | "gnu" => {
                personality.flags = Flags::GOOD_O | Flags::OLD_M;
                personality.prefer_bsd_defaults = true;
                personality.unix = UnixFormats {
                    f: Some("RD_f"),
                    fl: None,
                    j: Some("RD_j"),
                    l: Some("RD_l"),
                };
            }
            "linux" => {
                personality.flags = Flags::GOOD_O | Flags::ZAP_ADDR | Flags::SANE_USER;
            }
            "aix" => personality.bsd = FREEBSD,
            "tru64" | "compaq" | "digital" => {
                personality.flags = Flags::GOOD_O | Flags::BSD_H;
                personality.prefer_bsd_defaults = true;
                personality.unix = UnixFormats {
                    f: Some("F5FMT"),
                    fl: Some("FL5FMT"),
                    j: Some("JFMT"),
                    l: Some("L5FMT"),
                };
                personality.bsd = BsdFormats {
                    j: "JFMT",
                    l: "LFMT",
                    s: "SFMT",
                    u: "UFMT",
                    v: "VFMT",
                };
            }
            "sunos4" => {
                personality.flags = Flags::NO_DEFAULT_G;
                personality.prefer_bsd_defaults = true;
                personality.bsd = FREEBSD;
            }
            "irix" | "sgi" => {
                let xpg = system.variable("_XPG");
                let xpg_digit = xpg
                    .as_ref()
                    .and_then(|value| value.as_bytes().first())
                    .is_some_and(|&first| (b'1'..=b'9').contains(&first));

                if !xpg_digit {
                    personality.flags = Flags::IRIX_L;
                }
            }
            "os390" | "s390" | "390" => personality.unix.j = Some("J390"),
            "hp" | "hpux" => personality.flags = Flags::HPUX_X,
            "svr4" | "sysv" | "sco" => personality.flags = Flags::SVR4_X,
            _ => {}
        }

        personality
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{BsdFormats, FREEBSD, Flags, OLD_LINUX, Personality, UnixFormats};
    use crate::ps::system::testing::FakeSystem;

    fn personality(variables: &[(&'static str, &'static str)]) -> Personality {
        let system = FakeSystem {
            variables: HashMap::from_iter(variables.iter().copied()),
            ..FakeSystem::default()
        };

        Personality::from_environment(&system)
    }

    #[rstest]
    #[case::unset(&[], b"unknown")]
    #[case::empty(&[("PS_PERSONALITY", "")], b"unknown")]
    #[case::unknown(&[("PS_PERSONALITY", "foo")], b"foo")]
    #[case::cmd_env(&[("CMD_ENV", "posix")], b"posix")]
    #[case::too_long(&[("PS_PERSONALITY", "abcdefghijklmnop")], b"You found a bug!")]
    #[case::longest(&[("PS_PERSONALITY", "abcdefghijklmno")], b"abcdefghijklmno")]
    fn a_personality_without_changes_keeps_its_name(
        #[case] variables: &[(&'static str, &'static str)],
        #[case] name: &[u8],
    ) {
        assert_eq!(
            personality(variables),
            Personality {
                flags: Flags::empty(),
                prefer_bsd_defaults: false,
                bsd: OLD_LINUX,
                unix: UnixFormats::default(),
                name: name.to_vec(),
            }
        );
    }

    /// The personality called `name`, with `flags`, whether BSD's defaults are
    /// preferred, and the macros for its formats.
    fn changed(
        name: &str,
        flags: Flags,
        prefer_bsd_defaults: bool,
        bsd: BsdFormats,
        unix: UnixFormats,
    ) -> Personality {
        Personality {
            flags,
            prefer_bsd_defaults,
            bsd,
            unix,
            name: name.as_bytes().to_vec(),
        }
    }

    #[rstest]
    #[case::bsd(
        &[("PS_PERSONALITY", "BSD")],
        changed("BSD", Flags::FORCE_BSD | Flags::BSD_H | Flags::BSD_M, true, FREEBSD, UnixFormats::default())
    )]
    #[case::broken(
        &[("PS_PERSONALITY", "bsd"), ("I_WANT_A_BROKEN_PS", "")],
        changed("old", Flags::FORCE_BSD | Flags::OLD_M, true, OLD_LINUX, UnixFormats::default())
    )]
    #[case::gnu(
        &[("CMD_ENV", "gnu")],
        changed(
            "gnu",
            Flags::GOOD_O | Flags::OLD_M,
            true,
            OLD_LINUX,
            UnixFormats { f: Some("RD_f"), fl: None, j: Some("RD_j"), l: Some("RD_l") }
        )
    )]
    #[case::linux(
        &[("PS_PERSONALITY", "linux")],
        changed("linux", Flags::GOOD_O | Flags::ZAP_ADDR | Flags::SANE_USER, false, OLD_LINUX, UnixFormats::default())
    )]
    #[case::aix(&[("PS_PERSONALITY", "aix")], changed("aix", Flags::empty(), false, FREEBSD, UnixFormats::default()))]
    #[case::digital(
        &[("PS_PERSONALITY", "tru64")],
        changed(
            "tru64",
            Flags::GOOD_O | Flags::BSD_H,
            true,
            BsdFormats { j: "JFMT", l: "LFMT", s: "SFMT", u: "UFMT", v: "VFMT" },
            UnixFormats {
                f: Some("F5FMT"),
                fl: Some("FL5FMT"),
                j: Some("JFMT"),
                l: Some("L5FMT"),
            }
        )
    )]
    #[case::irix(&[("PS_PERSONALITY", "sgi")], changed("sgi", Flags::IRIX_L, false, OLD_LINUX, UnixFormats::default()))]
    #[case::irix_with_xpg(
        &[("PS_PERSONALITY", "irix"), ("_XPG", "2")],
        changed("irix", Flags::empty(), false, OLD_LINUX, UnixFormats::default())
    )]
    #[case::irix_with_xpg_zero(
        &[("PS_PERSONALITY", "irix"), ("_XPG", "0")],
        changed("irix", Flags::IRIX_L, false, OLD_LINUX, UnixFormats::default())
    )]
    #[case::os390(
        &[("PS_PERSONALITY", "390")],
        changed("390", Flags::empty(), false, OLD_LINUX, UnixFormats { j: Some("J390"), ..UnixFormats::default() })
    )]
    #[case::sunos(&[("PS_PERSONALITY", "sunos4")], changed("sunos4", Flags::NO_DEFAULT_G, true, FREEBSD, UnixFormats::default()))]
    #[case::hpux(&[("PS_PERSONALITY", "hp")], changed("hp", Flags::HPUX_X, false, OLD_LINUX, UnixFormats::default()))]
    #[case::svr4(&[("PS_PERSONALITY", "sco")], changed("sco", Flags::SVR4_X, false, OLD_LINUX, UnixFormats::default()))]
    fn a_personality_changes_the_defaults(
        #[case] variables: &[(&'static str, &'static str)],
        #[case] expected: Personality,
    ) {
        assert_eq!(personality(variables), expected);
    }
}
