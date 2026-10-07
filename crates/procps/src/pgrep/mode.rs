// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::ffi::OsStr;

/// Which tool runs: procps-ng builds `pgrep` and `pkill` from one source, and
/// the options and output differ between them.
///
/// ```
/// use procps::pgrep::Mode;
///
/// assert_ne!(Mode::Pgrep, Mode::Pkill);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// `pgrep`, which prints the processes that it selects.
    Pgrep,
    /// `pkill`, which signals the processes that it selects.
    Pkill,
}

impl Mode {
    /// The tool that procps-ng runs as when its short program name is
    /// `program`: `pkill` for `pkill` or `lt-pkill` (libtool's name for an
    /// uninstalled `pkill`), and `pgrep` for any other name. This project does
    /// not implement procps-ng's `pidwait`, which runs under the name
    /// `pidwait` or `lt-pidwait`.
    ///
    /// ```
    /// use procps::pgrep::Mode;
    ///
    /// assert_eq!(Mode::from_program("pkill".as_ref()), Mode::Pkill);
    /// ```
    #[must_use]
    pub fn from_program(program: &OsStr) -> Self {
        if program == "pkill" || program == "lt-pkill" {
            return Self::Pkill;
        }

        Self::Pgrep
    }

    /// The tool's name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pgrep => "pgrep",
            Self::Pkill => "pkill",
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::Mode;

    #[rstest]
    #[case::pkill("pkill", Mode::Pkill)]
    #[case::libtool("lt-pkill", Mode::Pkill)]
    #[case::pgrep("pgrep", Mode::Pgrep)]
    #[case::other("pkill2", Mode::Pgrep)]
    fn the_program_name_decides_the_mode(#[case] program: &str, #[case] expected: Mode) {
        assert_eq!(Mode::from_program(program.as_ref()), expected);
    }
}
