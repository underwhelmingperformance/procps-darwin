// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::BTreeMap, fmt, os::unix::process::ExitStatusExt, path::Path, process::Output,
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::{Error, pids::PidNames};

/// What a command did: its exit status, what it wrote, and what happened to
/// the scenario's fixtures.
///
/// A golden file contains one outcome as TOML:
///
/// ```
/// use procps_harness::{FixtureEnd, Outcome};
///
/// let outcome: Outcome = r#"
/// status = 0
/// stdout = "{a1}\n"
/// stderr = ""
///
/// [fixtures]
/// a1 = "killed by SIGTERM"
/// "#
/// .parse()?;
///
/// assert_eq!(
///     outcome.fixtures["a1"],
///     FixtureEnd::Killed("SIGTERM".to_owned())
/// );
/// # Ok::<(), toml::de::Error>(())
/// ```
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    /// The exit status, or 128 plus the signal number for a command that a
    /// signal terminated, as a shell reports it.
    pub status: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// The state of each fixture, by name, when the command had finished.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fixtures: BTreeMap<String, FixtureEnd>,
}

impl TryFrom<Output> for Outcome {
    type Error = Error;

    fn try_from(output: Output) -> Result<Self, Self::Error> {
        let status = output
            .status
            .code()
            .or_else(|| output.status.signal().map(|signal| 128 + signal))
            .unwrap_or(-1);
        let text = |stream: Stream, bytes: Vec<u8>| {
            String::from_utf8(bytes).map_err(|_| Error::NotUtf8 { stream })
        };

        Ok(Self {
            status,
            stdout: text(Stream::Stdout, output.stdout)?,
            stderr: text(Stream::Stderr, output.stderr)?,
            fixtures: BTreeMap::new(),
        })
    }
}

impl FromStr for Outcome {
    type Err = toml::de::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        toml::from_str(text)
    }
}

impl Outcome {
    /// The outcome with each known pid in its output replaced by its name.
    pub(crate) fn masked(self, names: &PidNames) -> Self {
        Self {
            stdout: names.mask(&self.stdout),
            stderr: names.mask(&self.stderr),
            ..self
        }
    }

    /// Reads an outcome from a golden file.
    ///
    /// ```
    /// use procps_harness::Outcome;
    ///
    /// let directory = tempfile::tempdir()?;
    /// let path = directory.path().join("by-name.toml");
    /// let outcome = Outcome {
    ///     status: 1,
    ///     ..Outcome::default()
    /// };
    /// outcome.write(&path)?;
    ///
    /// assert_eq!(Outcome::read(&path)?, outcome);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not an outcome.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;

        text.parse().map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Writes the outcome to a golden file, replacing any file already there.
    ///
    /// ```
    /// use procps_harness::Outcome;
    ///
    /// let directory = tempfile::tempdir()?;
    /// let path = directory.path().join("pgrep").join("by-name.toml");
    ///
    /// Outcome::default().write(&path)?;
    ///
    /// assert_eq!(
    ///     std::fs::read_to_string(&path)?,
    ///     "status = 0\nstdout = \"\"\nstderr = \"\"\n"
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn write(&self, path: &Path) -> Result<(), Error> {
        let text = toml::to_string_pretty(self)?;
        let io_error = |source| Error::Io {
            path: path.to_owned(),
            source,
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }

        std::fs::write(path, text).map_err(io_error)
    }
}

/// One of a command's output streams.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl fmt::Display for Stream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "standard output",
            Self::Stderr => "standard error",
        })
    }
}

/// What had happened to a fixture by the time the command finished.
///
/// A golden file records it as `running`, `killed by <signal>` or
/// `stopped by <signal>`, with the signal's name:
///
/// ```
/// use procps_harness::FixtureEnd;
///
/// let end: FixtureEnd = "stopped by SIGSTOP".parse()?;
///
/// assert_eq!(end, FixtureEnd::Stopped("SIGSTOP".to_owned()));
/// assert_eq!(end.to_string(), "stopped by SIGSTOP");
/// # Ok::<(), procps_harness::Error>(())
/// ```
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(into = "String", try_from = "String")]
pub enum FixtureEnd {
    /// The fixture was still running.
    Running,
    /// A signal, such as `SIGTERM`, had terminated the fixture.
    Killed(String),
    /// A signal, such as `SIGSTOP`, had stopped the fixture.
    Stopped(String),
}

const KILLED_BY: &str = "killed by ";
const STOPPED_BY: &str = "stopped by ";

impl fmt::Display for FixtureEnd {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => formatter.write_str("running"),
            Self::Killed(signal) => write!(formatter, "{KILLED_BY}{signal}"),
            Self::Stopped(signal) => write!(formatter, "{STOPPED_BY}{signal}"),
        }
    }
}

impl FromStr for FixtureEnd {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text == "running" {
            return Ok(Self::Running);
        }

        if let Some(signal) = text.strip_prefix(KILLED_BY) {
            return Ok(Self::Killed(signal.to_owned()));
        }

        if let Some(signal) = text.strip_prefix(STOPPED_BY) {
            return Ok(Self::Stopped(signal.to_owned()));
        }

        Err(Error::UnknownFixtureEnd(text.to_owned()))
    }
}

impl From<FixtureEnd> for String {
    fn from(end: FixtureEnd) -> Self {
        end.to_string()
    }
}

impl TryFrom<String> for FixtureEnd {
    type Error = Error;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        os::unix::process::ExitStatusExt,
        process::{ExitStatus, Output},
    };

    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{FixtureEnd, Outcome, Stream};
    use crate::{Error, pids::PidNames};

    #[rstest]
    #[case::exit_code(3 << 8, 3)]
    #[case::signal(15, 128 + 15)]
    fn an_outcome_records_the_exit_status_as_a_shell_does(
        #[case] raw: i32,
        #[case] status: i32,
    ) -> Result<(), Error> {
        let output = Output {
            status: ExitStatus::from_raw(raw),
            stdout: b"out\n".to_vec(),
            stderr: b"err\n".to_vec(),
        };

        assert_eq!(
            Outcome::try_from(output)?,
            Outcome {
                status,
                stdout: "out\n".to_owned(),
                stderr: "err\n".to_owned(),
                fixtures: BTreeMap::new(),
            }
        );

        Ok(())
    }

    #[test]
    fn output_that_is_not_utf8_is_an_error() {
        let output = Output {
            status: ExitStatus::from_raw(0),
            stdout: vec![0xff],
            stderr: Vec::new(),
        };

        assert_matches!(
            Outcome::try_from(output),
            Err(Error::NotUtf8 {
                stream: Stream::Stdout,
                ..
            })
        );
    }

    #[test]
    fn masking_replaces_pids_in_both_streams() {
        let names: PidNames = [("a1".to_owned(), 43210)].into_iter().collect();
        let outcome = Outcome {
            status: 0,
            stdout: "43210\n".to_owned(),
            stderr: "pgrep: 43210 failed\n".to_owned(),
            fixtures: BTreeMap::new(),
        };

        assert_eq!(
            outcome.masked(&names),
            Outcome {
                status: 0,
                stdout: "{a1}\n".to_owned(),
                stderr: "pgrep: {a1} failed\n".to_owned(),
                fixtures: BTreeMap::new(),
            }
        );
    }

    #[rstest]
    #[case::running("running", FixtureEnd::Running)]
    #[case::killed("killed by SIGTERM", FixtureEnd::Killed("SIGTERM".to_owned()))]
    #[case::stopped("stopped by SIGSTOP", FixtureEnd::Stopped("SIGSTOP".to_owned()))]
    fn a_fixture_end_round_trips_through_its_text(
        #[case] text: &str,
        #[case] end: FixtureEnd,
    ) -> Result<(), Error> {
        assert_eq!(
            (text.parse::<FixtureEnd>()?, end.to_string()),
            (end, text.to_owned())
        );

        Ok(())
    }

    #[test]
    fn an_unknown_fixture_end_is_an_error() {
        assert_matches!(
            "exploded".parse::<FixtureEnd>(),
            Err(Error::UnknownFixtureEnd(text)) if text == "exploded"
        );
    }

    #[test]
    fn an_outcome_survives_a_round_trip_through_a_golden_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("pkill").join("by-name.toml");
        let outcome = Outcome {
            status: 1,
            stdout: "{a}\n{b}\n".to_owned(),
            stderr: "pgrep: no matching criteria specified\n".to_owned(),
            fixtures: [
                ("a".to_owned(), FixtureEnd::Killed("SIGTERM".to_owned())),
                ("b".to_owned(), FixtureEnd::Running),
            ]
            .into_iter()
            .collect(),
        };

        outcome.write(&path)?;

        assert_eq!(Outcome::read(&path)?, outcome);

        Ok(())
    }
}
