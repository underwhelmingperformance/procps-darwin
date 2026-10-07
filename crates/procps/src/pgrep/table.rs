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

use crate::pgrep::Mode;

/// An option of `pgrep` or `pkill` that takes no argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum FlagId {
    IgnoreAncestors,
    RequireHandler,
    Count,
    ListName,
    ListFull,
    Full,
    IgnoreCase,
    Newest,
    Oldest,
    Inverse,
    Lightweight,
    Exact,
    Logpidfile,
    Echo,
    Quiet,
    ShellQuote,
    Mrelease,
    Help,
    Version,
    Question,
}

/// An option of `pgrep` or `pkill` that takes an argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum ValueId {
    Signal,
    Cgroup,
    Delimiter,
    Pgroup,
    Group,
    Older,
    Pid,
    Parent,
    Session,
    Terminal,
    Euid,
    Uid,
    Pidfile,
    Ns,
    Nslist,
    Queue,
    Runstates,
    Env,
}

/// An option, which takes an argument or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OptionId {
    Flag(FlagId),
    Value(ValueId),
}

/// Whether an option takes an argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Takes {
    Nothing,
    Argument,
}

/// The tools that accept an option's short form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tools {
    Both,
    Only(Mode),
}

/// One option: its long and short forms.
#[derive(Clone, Copy, Debug)]
pub(super) struct Spec {
    pub(super) id: OptionId,
    pub(super) long: Option<&'static str>,
    short: Option<(u8, Tools)>,
}

impl Spec {
    const fn flag(id: FlagId, long: Option<&'static str>, short: Option<(u8, Tools)>) -> Self {
        Self {
            id: OptionId::Flag(id),
            long,
            short,
        }
    }

    const fn value(id: ValueId, long: &'static str, short: Option<(u8, Tools)>) -> Self {
        Self {
            id: OptionId::Value(id),
            long: Some(long),
            short,
        }
    }

    /// Whether the option takes an argument.
    pub(super) const fn takes(&self) -> Takes {
        match self.id {
            OptionId::Flag(_) => Takes::Nothing,
            OptionId::Value(_) => Takes::Argument,
        }
    }

    /// The option's short form in the tool that `mode` selects, if that tool
    /// has one.
    pub(super) fn short(&self, mode: Mode) -> Option<u8> {
        match self.short? {
            (short, Tools::Both) => Some(short),
            (short, Tools::Only(only)) if only == mode => Some(short),
            (_, Tools::Only(_)) => None,
        }
    }

    /// The ID of the option's `clap` argument.
    pub(super) fn clap_id(&self) -> &'static str {
        self.long.unwrap_or("question")
    }
}

const PGREP: Tools = Tools::Only(Mode::Pgrep);
const PKILL: Tools = Tools::Only(Mode::Pkill);
const BOTH: Tools = Tools::Both;

/// Every option, with the long options in the order of procps-ng's `longopts`
/// table. glibc lists the possibilities of an ambiguous abbreviation in this
/// order. Both tools accept every long option, as in procps-ng, but each tool
/// accepts only its own short options.
pub(super) const OPTIONS: [Spec; 38] = {
    use FlagId as F;
    use ValueId as V;

    [
        Spec::value(V::Signal, "signal", None),
        Spec::flag(
            F::IgnoreAncestors,
            Some("ignore-ancestors"),
            Some((b'A', BOTH)),
        ),
        Spec::flag(
            F::RequireHandler,
            Some("require-handler"),
            Some((b'H', BOTH)),
        ),
        Spec::flag(F::Count, Some("count"), Some((b'c', BOTH))),
        Spec::value(V::Cgroup, "cgroup", None),
        Spec::value(V::Delimiter, "delimiter", Some((b'd', PGREP))),
        Spec::flag(F::ListName, Some("list-name"), Some((b'l', PGREP))),
        Spec::flag(F::ListFull, Some("list-full"), Some((b'a', PGREP))),
        Spec::flag(F::Full, Some("full"), Some((b'f', BOTH))),
        Spec::value(V::Pgroup, "pgroup", Some((b'g', BOTH))),
        Spec::value(V::Group, "group", Some((b'G', BOTH))),
        Spec::flag(F::IgnoreCase, Some("ignore-case"), Some((b'i', BOTH))),
        Spec::flag(F::Newest, Some("newest"), Some((b'n', BOTH))),
        Spec::flag(F::Oldest, Some("oldest"), Some((b'o', BOTH))),
        Spec::value(V::Older, "older", Some((b'O', BOTH))),
        Spec::value(V::Pid, "pid", Some((b'p', BOTH))),
        Spec::value(V::Parent, "parent", Some((b'P', BOTH))),
        Spec::value(V::Session, "session", Some((b's', BOTH))),
        Spec::value(V::Terminal, "terminal", Some((b't', BOTH))),
        Spec::value(V::Euid, "euid", Some((b'u', BOTH))),
        Spec::value(V::Uid, "uid", Some((b'U', BOTH))),
        Spec::flag(F::Inverse, Some("inverse"), Some((b'v', PGREP))),
        Spec::flag(F::Lightweight, Some("lightweight"), Some((b'w', PGREP))),
        Spec::flag(F::Exact, Some("exact"), Some((b'x', BOTH))),
        Spec::value(V::Pidfile, "pidfile", Some((b'F', BOTH))),
        Spec::flag(F::Logpidfile, Some("logpidfile"), Some((b'L', BOTH))),
        Spec::flag(F::Echo, Some("echo"), Some((b'e', PKILL))),
        Spec::value(V::Ns, "ns", None),
        Spec::value(V::Nslist, "nslist", None),
        Spec::value(V::Queue, "queue", Some((b'q', PKILL))),
        Spec::flag(F::Quiet, Some("quiet"), None),
        Spec::value(V::Runstates, "runstates", Some((b'r', BOTH))),
        Spec::value(V::Env, "env", None),
        Spec::flag(F::ShellQuote, Some("shell-quote"), Some((b'Q', BOTH))),
        Spec::flag(F::Mrelease, Some("mrelease"), Some((b'm', PKILL))),
        Spec::flag(F::Help, Some("help"), Some((b'h', BOTH))),
        Spec::flag(F::Version, Some("version"), Some((b'V', BOTH))),
        Spec::flag(F::Question, None, Some((b'?', BOTH))),
    ]
};

/// The option whose short form in the tool that `mode` selects is `short`.
pub(super) fn by_short(mode: Mode, short: u8) -> Option<&'static Spec> {
    OPTIONS.iter().find(|spec| spec.short(mode) == Some(short))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{OPTIONS, Takes};
    use crate::pgrep::Mode;

    /// The short options of `mode`, sorted, each with whether it takes an
    /// argument.
    fn short_options(mode: Mode) -> Vec<(u8, bool)> {
        let mut options: Vec<(u8, bool)> = OPTIONS
            .iter()
            .filter_map(|spec| {
                let short = spec.short(mode)?;

                Some((short, spec.takes() == Takes::Argument))
            })
            .collect();
        options.sort_unstable();

        options
    }

    /// The options in a `getopt` option string, sorted, each with whether it
    /// takes an argument.
    fn parse_optstring(optstring: &str) -> Vec<(u8, bool)> {
        let bytes = optstring.as_bytes();
        let mut options: Vec<(u8, bool)> = bytes
            .iter()
            .enumerate()
            .filter(|&(_, &byte)| byte != b':')
            .map(|(index, &byte)| (byte, bytes.get(index + 1) == Some(&b':')))
            .collect();
        options.sort_unstable();

        options
    }

    #[rstest]
    #[case::pgrep(Mode::Pgrep, "lad:vwQLF:cfinoxp:P:O:AHg:s:u:U:G:t:r:?Vh")]
    #[case::pkill(Mode::Pkill, "eq:mQLF:cfinoxp:P:O:AHg:s:u:U:G:t:r:?Vh")]
    fn the_short_options_are_procps_ng_s(#[case] mode: Mode, #[case] optstring: &str) {
        assert_eq!(short_options(mode), parse_optstring(optstring));
    }
}
