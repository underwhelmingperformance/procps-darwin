// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    num::NonZeroUsize,
    ops::RangeInclusive,
    process::{Command, Stdio},
    sync::LazyLock,
};

use regex::Regex;

use crate::{Error, spawn};

/// The pids that a run accepts for its session leader, fixtures and command.
///
/// Masking replaces a pid with a name but leaves the spaces before it. A
/// padded column therefore keeps the same layout on every run only if every
/// masked pid has the same number of digits. macOS allocates pids up to
/// 99999, so five digits are possible on both systems.
pub(crate) const MASKED_PIDS: RangeInclusive<u32> = 10_000..=99_999;

/// Starts and reaps processes until the system's next pid is at least the
/// start of [`MASKED_PIDS`].
///
/// The pids of a freshly started container, or of a recently booted Mac, are
/// below that range, and so are the pids after the counter wraps. Every thread
/// of this process waits while one of them skips pids, and each first reads
/// the current pid, so several threads do not skip far enough to wrap the pid
/// counter.
pub(crate) fn skip_low_pids() -> Result<(), Error> {
    let _spawning = spawn::lock();
    let shell = || {
        let mut command = Command::new(SHELL);
        command.stdin(Stdio::null()).stderr(Stdio::null());
        command
    };
    let io_error = |source| Error::Io {
        path: SHELL.into(),
        source,
    };

    let output = shell().args(["-c", "echo $$"]).output().map_err(io_error)?;
    let current: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| Error::SkipPids(output.status))?;

    let Some(count) = MASKED_PIDS.start().checked_sub(current) else {
        return Ok(());
    };

    let processors = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let mut started = Vec::new();
    let mut failure = None;

    for share in shares(count, processors)
        .into_iter()
        .filter(|&share| share > 0)
    {
        // Each `(:)` forks a subshell without executing a program.
        let spawned = shell()
            .args([
                "-c",
                r#"i=0; while [ "$i" -lt "$1" ]; do (:); i=$((i + 1)); done"#,
                "sh",
                &share.to_string(),
            ])
            .spawn();

        match spawned {
            Ok(child) => started.push(child),
            Err(source) => {
                failure = Some(io_error(source));
                break;
            }
        }
    }

    for mut child in started {
        match child.wait() {
            Ok(status) if !status.success() => {
                failure.get_or_insert(Error::SkipPids(status));
            }
            Ok(_) => {}
            Err(source) => {
                failure.get_or_insert(io_error(source));
            }
        }
    }

    failure.map_or(Ok(()), Err)
}

/// `count` split into `shells` parts whose sizes differ by at most one.
fn shares(count: u32, shells: usize) -> Vec<u32> {
    let shells = u32::try_from(shells.max(1)).unwrap_or(u32::MAX);

    (0..shells)
        .map(|index| count / shells + u32::from(index < count % shells))
        .collect()
}

const SHELL: &str = "/bin/sh";

/// Known pids and the names that masked output shows for them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PidNames(Vec<(String, u32)>);

impl PidNames {
    /// The pid with the name `name`.
    pub(crate) fn get(&self, name: &str) -> Option<u32> {
        self.0
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, pid)| *pid)
    }

    /// Replaces each known pid in `text` with its name in braces.
    pub(crate) fn mask(&self, text: &str) -> String {
        NUMBER
            .replace_all(text, |captures: &regex::Captures<'_>| {
                let name = captures[0].parse().ok().and_then(|pid| self.name_of(pid));

                name.map_or_else(|| captures[0].to_owned(), |name| format!("{{{name}}}"))
            })
            .into_owned()
    }

    fn name_of(&self, pid: u32) -> Option<&str> {
        self.0
            .iter()
            .find(|(_, known)| *known == pid)
            .map(|(name, _)| name.as_str())
    }
}

/// A decimal number that is not part of a longer word.
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+\b").expect("the pattern is valid"));

impl FromIterator<(String, u32)> for PidNames {
    fn from_iter<T: IntoIterator<Item = (String, u32)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Extend<(String, u32)> for PidNames {
    fn extend<T: IntoIterator<Item = (String, u32)>>(&mut self, iter: T) {
        self.0.extend(iter);
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{PidNames, shares};

    fn names() -> PidNames {
        [("a".to_owned(), 12345), ("b".to_owned(), 45678)]
            .into_iter()
            .collect()
    }

    #[rstest]
    #[case::bare_pids("12345\n45678\n", "{a}\n{b}\n")]
    #[case::padded_column(
        "    PID COMMAND\n  12345 alpha\n  45678 beta\n",
        "    PID COMMAND\n  {a} alpha\n  {b} beta\n"
    )]
    #[case::comma_list("12345,45678\n", "{a},{b}\n")]
    #[case::inside_a_longer_number("123456 812345 45678\n", "123456 812345 {b}\n")]
    #[case::unrelated_text("no pids here\n", "no pids here\n")]
    fn pids_are_replaced_by_names(#[case] text: &str, #[case] expected: &str) {
        assert_eq!(names().mask(text), expected);
    }

    #[rstest]
    #[case::uneven(10, 4, vec![3, 3, 2, 2])]
    #[case::fewer_than_the_shells(2, 4, vec![1, 1, 0, 0])]
    #[case::one_shell(5, 1, vec![5])]
    #[case::none(0, 2, vec![0, 0])]
    fn the_pids_to_skip_are_shared_between_the_shells(
        #[case] count: u32,
        #[case] shells: usize,
        #[case] expected: Vec<u32>,
    ) {
        assert_eq!(shares(count, shells), expected);
    }

    #[rstest]
    #[case::known("b", Some(45678))]
    #[case::unknown("c", None)]
    fn a_pid_is_found_by_name(#[case] name: &str, #[case] expected: Option<u32>) {
        assert_eq!(names().get(name), expected);
    }
}
