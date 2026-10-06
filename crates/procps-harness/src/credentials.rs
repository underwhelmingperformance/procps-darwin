// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{os::unix::process::CommandExt, process::Command, str::FromStr};

use crate::Error;

/// The user and group to run a scenario's processes as.
///
/// A runner that runs as root uses these so procps-ng sees the same
/// unprivileged view of the process table as a user on macOS.
///
/// ```
/// use procps_harness::Credentials;
///
/// let credentials: Credentials = "1000:100".parse()?;
///
/// assert_eq!(
///     credentials,
///     Credentials {
///         uid: 1000,
///         gid: 100
///     }
/// );
/// # Ok::<(), procps_harness::Error>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Credentials {
    /// The user id.
    pub uid: u32,
    /// The group id.
    pub gid: u32,
}

impl Credentials {
    /// Makes `command` run as this user and group.
    pub(crate) fn apply(self, command: &mut Command) -> &mut Command {
        command.uid(self.uid).gid(self.gid)
    }
}

impl FromStr for Credentials {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidCredentials(text.to_owned());
        let (uid, gid) = text.split_once(':').ok_or_else(invalid)?;

        Ok(Self {
            uid: uid.parse().map_err(|_| invalid())?,
            gid: gid.parse().map_err(|_| invalid())?,
        })
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use rstest::rstest;

    use super::Credentials;
    use crate::Error;

    #[rstest]
    #[case::no_group("1000")]
    #[case::not_a_number("user:100")]
    #[case::empty("")]
    fn malformed_credentials_are_rejected(#[case] text: &str) {
        assert_matches!(
            text.parse::<Credentials>(),
            Err(Error::InvalidCredentials(found)) if found == text
        );
    }
}
