// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::{Error, pids::PidNames};

/// The name of the session leader's pid in commands and masked output.
pub(crate) const SESSION: &str = "session";

/// The name of the command's pid in masked output.
pub(crate) const COMMAND: &str = "command";

/// The longest process name that Linux reports, without its terminating NUL.
const COMM_MAX: usize = 15;

/// A process for a scenario to start before it runs its command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureSpec {
    /// The name that the command and the masked output use for the process.
    pub(crate) name: String,
    /// The process name that the system reports.
    pub(crate) comm: String,
}

/// One command to run in a new session, with the fixture processes that the
/// session contains.
///
/// The command runs in the same session as the fixtures, and it must select
/// processes only from that session, for example with `pgrep -s 0`. Other
/// scenarios and decoys run at the same time with the same process names, in
/// other sessions.
///
/// ```
/// use procps_harness::Scenario;
///
/// let scenario: Scenario = r#"
/// description = "pgrep prints the pids of the processes with a name"
/// command = ["pgrep", "-s", "0", "harness-alpha"]
///
/// [[fixture]]
/// name = "a1"
/// comm = "harness-alpha"
/// "#
/// .parse()?;
///
/// assert!(!scenario.is_pending());
/// # Ok::<(), toml::de::Error>(())
/// ```
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "ScenarioDocument", into = "ScenarioDocument")]
pub struct Scenario {
    description: String,
    pending: bool,
    pub(crate) command: Vec<Argument>,
    pub(crate) fixtures: Vec<FixtureSpec>,
}

impl Scenario {
    /// Whether this project's tool is still expected to differ from the golden
    /// file.
    ///
    /// ```
    /// use procps_harness::Scenario;
    ///
    /// let scenario: Scenario =
    ///     "description = \"pgrep -V\"\npending = true\ncommand = [\"pgrep\", \"-V\"]\n".parse()?;
    ///
    /// assert!(scenario.is_pending());
    /// # Ok::<(), toml::de::Error>(())
    /// ```
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending
    }
}

impl FromStr for Scenario {
    type Err = toml::de::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        toml::from_str(text)
    }
}

/// A scenario file as written, before it is checked.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScenarioDocument {
    description: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pending: bool,
    command: Vec<String>,
    #[serde(default, rename = "fixture", skip_serializing_if = "Vec::is_empty")]
    fixtures: Vec<FixtureSpec>,
}

/// Why a scenario file is not a valid scenario.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum ScenarioError {
    #[error("the command is empty")]
    NoCommand,
    #[error(
        "fixture name {0:?} must be letters, digits, `-` and `_`, and not {SESSION:?} or \
         {COMMAND:?}"
    )]
    FixtureName(String),
    #[error("fixture name {0:?} is used twice")]
    DuplicateFixture(String),
    #[error("comm {0:?} must have 1 to {COMM_MAX} bytes and no `/`")]
    Comm(String),
    #[error("{argument:?} has an unmatched `{{` or `}}`; write `{{{{` or `}}}}` for a brace")]
    UnmatchedBrace { argument: String },
    #[error("{argument:?} refers to {name:?}, which is not a fixture or {SESSION:?}")]
    UnknownPid { argument: String, name: String },
}

impl TryFrom<ScenarioDocument> for Scenario {
    type Error = ScenarioError;

    fn try_from(document: ScenarioDocument) -> Result<Self, Self::Error> {
        if document.command.is_empty() {
            return Err(ScenarioError::NoCommand);
        }

        let mut names = HashSet::from([SESSION]);

        for fixture in &document.fixtures {
            let valid_name = !fixture.name.is_empty()
                && fixture
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

            if !valid_name || fixture.name == SESSION || fixture.name == COMMAND {
                return Err(ScenarioError::FixtureName(fixture.name.clone()));
            }

            if !names.insert(&fixture.name) {
                return Err(ScenarioError::DuplicateFixture(fixture.name.clone()));
            }

            if fixture.comm.is_empty()
                || fixture.comm.len() > COMM_MAX
                || fixture.comm.contains('/')
            {
                return Err(ScenarioError::Comm(fixture.comm.clone()));
            }
        }

        let command = document
            .command
            .iter()
            .map(|text| {
                let argument: Argument = text.parse()?;
                let unknown = argument
                    .pids()
                    .find(|name| !names.contains(name))
                    .map(str::to_owned);

                match unknown {
                    Some(name) => Err(ScenarioError::UnknownPid {
                        argument: text.clone(),
                        name,
                    }),
                    None => Ok(argument),
                }
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            description: document.description,
            pending: document.pending,
            command,
            fixtures: document.fixtures,
        })
    }
}

impl From<Scenario> for ScenarioDocument {
    fn from(scenario: Scenario) -> Self {
        Self {
            description: scenario.description,
            pending: scenario.pending,
            command: scenario.command.iter().map(ToString::to_string).collect(),
            fixtures: scenario.fixtures,
        }
    }
}

/// A command argument: text with references to pids, written `{name}`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Argument(Vec<Part>);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Part {
    Text(String),
    Pid(String),
}

impl Argument {
    fn pids(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|part| match part {
            Part::Pid(name) => Some(name.as_str()),
            Part::Text(_) => None,
        })
    }

    /// The argument with each reference replaced by the pid in `names`.
    pub(crate) fn with_pids(&self, names: &PidNames) -> Result<String, Error> {
        self.0
            .iter()
            .map(|part| match part {
                Part::Text(text) => Ok(text.clone()),
                Part::Pid(name) => names.get(name).map(|pid| pid.to_string()).ok_or_else(|| {
                    Error::UnknownFixture {
                        argument: self.to_string(),
                        name: name.clone(),
                    }
                }),
            })
            .collect()
    }
}

impl FromStr for Argument {
    type Err = ScenarioError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let unmatched = || ScenarioError::UnmatchedBrace {
            argument: text.to_owned(),
        };
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut characters = text.chars().peekable();

        while let Some(character) = characters.next() {
            match character {
                '{' if characters.next_if_eq(&'{').is_some() => literal.push('{'),
                '}' if characters.next_if_eq(&'}').is_some() => literal.push('}'),
                '{' => {
                    let mut name = String::new();
                    let mut closed = false;

                    for character in characters.by_ref() {
                        if character == '}' {
                            closed = true;
                            break;
                        }

                        name.push(character);
                    }

                    if !closed || name.is_empty() || name.contains('{') {
                        return Err(unmatched());
                    }

                    if !literal.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut literal)));
                    }

                    parts.push(Part::Pid(name));
                }
                '}' => return Err(unmatched()),
                _ => literal.push(character),
            }
        }

        if !literal.is_empty() {
            parts.push(Part::Text(literal));
        }

        Ok(Self(parts))
    }
}

impl fmt::Display for Argument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for part in &self.0 {
            match part {
                Part::Text(text) => {
                    formatter.write_str(&text.replace('{', "{{").replace('}', "}}"))?;
                }
                Part::Pid(name) => write!(formatter, "{{{name}}}")?,
            }
        }

        Ok(())
    }
}

/// A scenario together with where it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScenarioFile {
    /// The tool, which is the name of the scenario's directory.
    pub tool: String,
    /// The scenario's file name without `.toml`.
    pub name: String,
    /// The scenario.
    pub scenario: Scenario,
}

impl ScenarioFile {
    /// Reads every `<tool>/<name>.toml` scenario under `directory`, sorted by
    /// tool and name.
    ///
    /// ```
    /// use procps_harness::ScenarioFile;
    ///
    /// let directory = tempfile::tempdir()?;
    /// std::fs::create_dir(directory.path().join("pgrep"))?;
    /// std::fs::write(
    ///     directory.path().join("pgrep").join("version.toml"),
    ///     "description = \"pgrep -V\"\ncommand = [\"pgrep\", \"-V\"]\n",
    /// )?;
    ///
    /// let files = ScenarioFile::discover(directory.path())?;
    ///
    /// assert_eq!(files[0].name, "version");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if a directory or scenario cannot be read or parsed.
    pub fn discover(directory: &Path) -> Result<Vec<Self>, Error> {
        toml_files(directory)?
            .into_iter()
            .map(|file| {
                let text = std::fs::read_to_string(&file.path).map_err(|source| Error::Io {
                    path: file.path.clone(),
                    source,
                })?;
                let scenario = text.parse().map_err(|source| Error::Parse {
                    path: file.path.clone(),
                    source,
                })?;

                Ok(Self {
                    tool: file.tool,
                    name: file.name,
                    scenario,
                })
            })
            .collect()
    }

    /// The golden file for this scenario under `golden`.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use procps_harness::ScenarioFile;
    ///
    /// let file = ScenarioFile {
    ///     tool: "pgrep".to_owned(),
    ///     name: "version".to_owned(),
    ///     scenario: "description = \"pgrep -V\"\ncommand = [\"pgrep\", \"-V\"]\n".parse()?,
    /// };
    ///
    /// assert_eq!(
    ///     file.golden_path(Path::new("golden")),
    ///     Path::new("golden/pgrep/version.toml")
    /// );
    /// # Ok::<(), toml::de::Error>(())
    /// ```
    #[must_use]
    pub fn golden_path(&self, golden: &Path) -> PathBuf {
        golden.join(&self.tool).join(format!("{}.toml", self.name))
    }
}

/// A `<tool>/<name>.toml` file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TomlFile {
    pub(crate) tool: String,
    pub(crate) name: String,
    pub(crate) path: PathBuf,
}

/// Lists every `<tool>/<name>.toml` file under `directory`, sorted by tool and
/// name.
pub(crate) fn toml_files(directory: &Path) -> Result<Vec<TomlFile>, Error> {
    let mut found = Vec::new();

    for tool_directory in sorted_entries(directory)? {
        if !tool_directory.is_dir() {
            continue;
        }

        for path in sorted_entries(&tool_directory)? {
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }

            found.push(TomlFile {
                tool: file_name(&tool_directory),
                name: path
                    .file_stem()
                    .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned()),
                path,
            });
        }
    }

    Ok(found)
}

fn sorted_entries(directory: &Path) -> Result<Vec<PathBuf>, Error> {
    let io_error = |source| Error::Io {
        path: directory.to_owned(),
        source,
    };
    let mut entries = std::fs::read_dir(directory)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error)?;

    entries.sort();

    Ok(entries)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Argument, FixtureSpec, Part, Scenario, ScenarioError, ScenarioFile, TomlFile};
    use crate::{Error, pids::PidNames};

    const BY_NAME: &str = r#"
description = "pgrep prints the pids of the processes with a name"
command = ["pgrep", "-s", "0", "harness-alpha"]

[[fixture]]
name = "a1"
comm = "harness-alpha"

[[fixture]]
name = "b1"
comm = "harness-beta"
"#;

    fn by_name() -> Result<Scenario, ScenarioError> {
        Ok(Scenario {
            description: "pgrep prints the pids of the processes with a name".to_owned(),
            pending: false,
            command: ["pgrep", "-s", "0", "harness-alpha"]
                .into_iter()
                .map(str::parse)
                .collect::<Result<_, _>>()?,
            fixtures: vec![
                FixtureSpec {
                    name: "a1".to_owned(),
                    comm: "harness-alpha".to_owned(),
                },
                FixtureSpec {
                    name: "b1".to_owned(),
                    comm: "harness-beta".to_owned(),
                },
            ],
        })
    }

    #[test]
    fn a_scenario_is_parsed_from_toml() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(BY_NAME.parse::<Scenario>()?, by_name()?);

        Ok(())
    }

    #[test]
    fn a_scenario_round_trips_through_toml() -> Result<(), Box<dyn std::error::Error>> {
        let scenario = by_name()?;

        assert_eq!(toml::to_string(&scenario)?.parse::<Scenario>()?, scenario);

        Ok(())
    }

    #[rstest]
    #[case::unknown_key("unexpected = true", "unknown field `unexpected`")]
    #[case::empty_command("command = []", "the command is empty")]
    #[case::reserved_name(
        "[[fixture]]\nname = \"session\"\ncomm = \"x\"",
        "fixture name \"session\""
    )]
    #[case::bad_name("[[fixture]]\nname = \"a b\"\ncomm = \"x\"", "fixture name \"a b\"")]
    #[case::duplicate_name(
        "[[fixture]]\nname = \"a1\"\ncomm = \"x\"",
        "fixture name \"a1\" is used twice"
    )]
    #[case::long_comm(
        "[[fixture]]\nname = \"c1\"\ncomm = \"sixteen-letters!\"",
        "comm \"sixteen-letters!\""
    )]
    #[case::comm_with_slash("[[fixture]]\nname = \"c1\"\ncomm = \"a/b\"", "comm \"a/b\"")]
    #[case::unknown_pid("command = [\"ps\", \"-p\", \"{zz}\"]", "refers to \"zz\"")]
    #[case::unmatched_close("command = [\"echo\", \"}\"]", "unmatched")]
    #[case::unclosed_reference("command = [\"echo\", \"{a1\"]", "unmatched")]
    fn an_invalid_scenario_is_rejected(#[case] change: &str, #[case] message: &str) {
        let text = if change.starts_with("command") {
            BY_NAME.replace(
                "command = [\"pgrep\", \"-s\", \"0\", \"harness-alpha\"]",
                change,
            )
        } else {
            format!("{BY_NAME}\n{change}\n")
        };

        assert_matches!(
            text.parse::<Scenario>(),
            Err(error) if error.message().contains(message),
            "expected an error containing {message:?}"
        );
    }

    #[rstest]
    #[case::plain("-x", vec![Part::Text("-x".to_owned())])]
    #[case::pid_list("{a1},{session}", vec![
        Part::Pid("a1".to_owned()),
        Part::Text(",".to_owned()),
        Part::Pid("session".to_owned()),
    ])]
    #[case::escaped_braces("{{a1}}", vec![Part::Text("{a1}".to_owned())])]
    fn an_argument_is_split_into_text_and_pids(
        #[case] text: &str,
        #[case] parts: Vec<Part>,
    ) -> Result<(), ScenarioError> {
        let argument: Argument = text.parse()?;

        assert_eq!(
            (argument.to_string(), argument),
            (text.to_owned(), Argument(parts))
        );

        Ok(())
    }

    #[test]
    fn references_become_pids() -> Result<(), Box<dyn std::error::Error>> {
        let names: PidNames = [("a1".to_owned(), 12345), ("b1".to_owned(), 23456)]
            .into_iter()
            .collect();
        let argument: Argument = "{a1},{b1}".parse()?;

        assert_eq!(argument.with_pids(&names)?, "12345,23456");

        Ok(())
    }

    #[test]
    fn a_reference_without_a_pid_is_an_error() -> Result<(), ScenarioError> {
        let argument: Argument = "{a1}".parse()?;

        assert_matches!(
            argument.with_pids(&PidNames::default()),
            Err(Error::UnknownFixture { argument, name }) if argument == "{a1}" && name == "a1"
        );

        Ok(())
    }

    #[test]
    fn scenarios_are_discovered_by_tool_and_name() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut expected = Vec::new();

        for (tool, name) in [
            ("pgrep", "anchored"),
            ("pgrep", "by-name"),
            ("ps", "pid-list"),
        ] {
            std::fs::create_dir_all(directory.path().join(tool))?;
            std::fs::write(
                directory.path().join(tool).join(format!("{name}.toml")),
                BY_NAME,
            )?;
            expected.push(ScenarioFile {
                tool: tool.to_owned(),
                name: name.to_owned(),
                scenario: by_name()?,
            });
        }

        std::fs::write(directory.path().join("ps").join("notes.txt"), "")?;

        assert_eq!(ScenarioFile::discover(directory.path())?, expected);

        Ok(())
    }

    #[test]
    fn toml_files_are_listed_by_tool_and_name() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        std::fs::create_dir(directory.path().join("ps"))?;
        std::fs::write(directory.path().join("ps").join("b.toml"), "")?;
        std::fs::write(directory.path().join("ps").join("a.toml"), "")?;
        std::fs::write(directory.path().join("top-level.toml"), "")?;

        assert_eq!(
            super::toml_files(directory.path())?,
            [
                TomlFile {
                    tool: "ps".to_owned(),
                    name: "a".to_owned(),
                    path: directory.path().join("ps").join("a.toml"),
                },
                TomlFile {
                    tool: "ps".to_owned(),
                    name: "b".to_owned(),
                    path: directory.path().join("ps").join("b.toml"),
                },
            ]
        );

        Ok(())
    }
}
