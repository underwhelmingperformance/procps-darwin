// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::LazyLock,
};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{Error, pids::PidNames};

/// The name of the session leader's pid in commands and masked output.
pub(crate) const SESSION: &str = "session";

/// The name of the command's pid in masked output.
pub(crate) const COMMAND: &str = "command";

/// The longest process name that Linux reports, without its terminating NUL.
const COMM_MAX: usize = 15;

/// The locales that a scenario may set. In the reference container, glibc has
/// its built-in `C` and `POSIX` locales and the `C.UTF-8` locale that nixpkgs
/// compiles into glibc's output, and no others, so procps-ng would fall back to
/// `C` for any other locale. glibc also accepts the name `C.utf8`, which macOS
/// lacks.
const LOCALES: [&str; 3] = ["C", "POSIX", "C.UTF-8"];

/// The time zones that a scenario may set: `UTC`, or a POSIX `TZ` string that
/// glibc 2.44 and macOS read in the same way.
///
/// The reference container has no time zone files, so glibc reads a zone name
/// such as `Europe/London` as UTC. For summer time without rules, glibc applies
/// the US rules because the container has no `posixrules` file, whereas macOS
/// reads its own `posixrules` file. A summer time zone therefore needs rules.
///
/// The two libraries read several rules that POSIX allows differently: a rule
/// time with a sign, the Julian and zero-based day rules, two rules whose order
/// can change from one year to the next, and a change of time close to the end
/// of a year. Summer time therefore needs two `M` rules in months 2 to 11, at
/// least two months apart, and a rule time is whole hours from 0 to 24 or hours
/// and minutes, with no sign or seconds. [`is_time_zone`] checks the months.
static TIME_ZONE: LazyLock<Regex> = LazyLock::new(|| {
    let name = "(?:[A-Za-z]{3,}|<[A-Za-z0-9+-]{3,}>)";
    let hours = "(?:[01]?[0-9]|2[0-4])";
    let offset = format!("[+-]?{hours}(?::[0-5][0-9]){{0,2}}");
    let time = "(?:(?:[01]?[0-9]|2[0-3])(?::[0-5][0-9])?|24(?::00)?)";
    let rule = |month| format!("M(?P<{month}>[2-9]|1[01])\\.[1-5]\\.[0-6](?:/{time})?");
    let (start, end) = (rule("start"), rule("end"));
    let pattern = format!("^(?:UTC|{name}{offset}(?:{name}(?:{offset})?,{start},{end})?)$");

    Regex::new(&pattern).expect("the time zone pattern is valid")
});

/// Whether `value` matches [`TIME_ZONE`]. If the value has rules for summer
/// time, the months of the two rules must also be at least two apart.
fn is_time_zone(value: &str) -> bool {
    let Some(captures) = TIME_ZONE.captures(value) else {
        return false;
    };
    let month = |name| {
        captures
            .name(name)
            .and_then(|month| month.as_str().parse::<u8>().ok())
    };

    match (month("start"), month("end")) {
        (Some(start), Some(end)) => start.abs_diff(end) >= 2,
        _ => true,
    }
}

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
    /// Variables for the command's environment. The session leader adds them
    /// after the harness's own variables, so a variable named `TZ` replaces the
    /// harness's value. The leader sets `LC_ALL=C` only if the table sets no
    /// locale variable.
    pub(crate) environment: BTreeMap<String, String>,
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

    /// Whether the `[environment]` table sets a locale variable: `LANG` or any
    /// `LC_` variable.
    pub(crate) fn sets_locale(&self) -> bool {
        self.environment.keys().any(|name| is_locale(name))
    }
}

/// Whether `name` is a locale variable: `LANG` or any `LC_` variable.
fn is_locale(name: &str) -> bool {
    name == "LANG" || name.starts_with("LC_")
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
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
    #[error("`[environment]` must not set {0}, because the harness sets it")]
    ReservedVariable(String),
    #[error("variable name {0:?} must not be empty or contain `=` or a NUL")]
    VariableName(String),
    #[error("the value of {0} must not contain a NUL")]
    VariableValue(String),
    #[error("the value of {name}, {value:?}, must be one of the locales {LOCALES:?}")]
    Locale { name: String, value: String },
    #[error(
        "the value of TZ, {0:?}, must be UTC or a POSIX TZ string such as JST-9 or \
         GMT0BST,M3.5.0/1,M10.5.0. A summer time zone needs two M rules in months 2 to 11, at \
         least two months apart, and a rule time must be from 0 to 24 hours, with minutes but \
         no seconds or sign"
    )]
    TimeZone(String),
}

impl TryFrom<ScenarioDocument> for Scenario {
    type Error = ScenarioError;

    fn try_from(document: ScenarioDocument) -> Result<Self, Self::Error> {
        if document.command.is_empty() {
            return Err(ScenarioError::NoCommand);
        }

        if let Some(reserved) = ["PATH", "HOME"]
            .into_iter()
            .find(|name| document.environment.contains_key(*name))
        {
            return Err(ScenarioError::ReservedVariable(reserved.to_owned()));
        }

        for (name, value) in &document.environment {
            if name.is_empty() || name.contains(['=', '\0']) {
                return Err(ScenarioError::VariableName(name.clone()));
            }

            if value.contains('\0') {
                return Err(ScenarioError::VariableValue(name.clone()));
            }

            if is_locale(name) && !LOCALES.contains(&value.as_str()) {
                return Err(ScenarioError::Locale {
                    name: name.clone(),
                    value: value.clone(),
                });
            }

            if name == "TZ" && !is_time_zone(value) {
                return Err(ScenarioError::TimeZone(value.clone()));
            }
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
            environment: document.environment,
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
            environment: scenario.environment,
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
    use std::collections::BTreeMap;

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
            environment: BTreeMap::new(),
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

    #[test]
    fn a_scenario_sets_variables_in_its_command_s_environment()
    -> Result<(), Box<dyn std::error::Error>> {
        let scenario: Scenario =
            format!(
                "{BY_NAME}\n[environment]\nPOSIXLY_CORRECT = \"1\"\nLC_ALL = \"C.UTF-8\"\nTZ = \"GMT0BST,M3.5.0/1,M10.5.0\"\n"
            )
                .parse()?;

        assert_eq!(
            scenario,
            Scenario {
                environment: [
                    ("LC_ALL", "C.UTF-8"),
                    ("POSIXLY_CORRECT", "1"),
                    ("TZ", "GMT0BST,M3.5.0/1,M10.5.0"),
                ]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
                ..by_name()?
            }
        );
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
    #[case::path("[environment]\nPATH = \"/bin\"", "must not set PATH")]
    #[case::home("[environment]\nHOME = \"/\"", "must not set HOME")]
    #[case::empty_variable_name("[environment]\n\"\" = \"x\"", "variable name \"\"")]
    #[case::variable_name_with_equals("[environment]\n\"A=B\" = \"x\"", "variable name \"A=B\"")]
    #[case::variable_name_with_nul("[environment]\n\"A\\u0000\" = \"x\"", "variable name \"A\\0\"")]
    #[case::value_with_nul("[environment]\nA = \"x\\u0000\"", "the value of A")]
    #[case::lang("[environment]\nLANG = \"en_GB.UTF-8\"", "the value of LANG")]
    #[case::locale_category("[environment]\nLC_CTYPE = \"en_GB.UTF-8\"", "the value of LC_CTYPE")]
    #[case::time_zone_name("[environment]\nTZ = \"Europe/London\"", "the value of TZ")]
    #[case::time_zone_file("[environment]\nTZ = \":UTC\"", "the value of TZ")]
    #[case::summer_time_without_rules("[environment]\nTZ = \"EST5EDT\"", "the value of TZ")]
    #[case::offset_too_large("[environment]\nTZ = \"ABC25\"", "the value of TZ")]
    #[case::month_out_of_range(
        "[environment]\nTZ = \"GMT0BST,M13.1.0,M10.5.0\"",
        "the value of TZ"
    )]
    #[case::week_out_of_range("[environment]\nTZ = \"GMT0BST,M3.9.0,M10.5.0\"", "the value of TZ")]
    #[case::rule_time_with_a_minus(
        "[environment]\nTZ = \"GMT0BST,M3.5.0/-1,M10.5.0\"",
        "the value of TZ"
    )]
    #[case::rule_time_with_a_plus(
        "[environment]\nTZ = \"GMT0BST,M3.5.0/+1,M10.5.0\"",
        "the value of TZ"
    )]
    #[case::zero_based_day_365("[environment]\nTZ = \"GMT0BST,0,365\"", "the value of TZ")]
    #[case::zero_based_days("[environment]\nTZ = \"AAA3BBB,0,364\"", "the value of TZ")]
    #[case::julian_days("[environment]\nTZ = \"AAA3BBB,J1/0,J365/24\"", "the value of TZ")]
    #[case::rule_in_december(
        "[environment]\nTZ = \"AAA3BBB,M3.2.0,M12.5.0/24\"",
        "the value of TZ"
    )]
    #[case::rule_in_january("[environment]\nTZ = \"AAA3BBB,M1.1.0/0,M11.1.0\"", "the value of TZ")]
    #[case::rules_in_one_month("[environment]\nTZ = \"AAA0BBB,M2.5.4,M2.5.5\"", "the value of TZ")]
    #[case::rules_in_adjacent_months(
        "[environment]\nTZ = \"AAA0BBB,M3.5.0,M4.1.0\"",
        "the value of TZ"
    )]
    #[case::rule_time_with_seconds(
        "[environment]\nTZ = \"GMT0BST,M3.5.0/1:00:00,M10.5.0\"",
        "the value of TZ"
    )]
    #[case::rule_time_after_24(
        "[environment]\nTZ = \"GMT0BST,M3.5.0/24:30,M10.5.0\"",
        "the value of TZ"
    )]
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
    #[case::utc("UTC")]
    #[case::offset("JST-9")]
    #[case::largest_offset("ABC-24:59:59")]
    #[case::northern_summer("GMT0BST,M3.5.0/1,M10.5.0")]
    #[case::southern_summer("AEST-10AEDT,M10.1.0,M4.1.0/3")]
    #[case::months_two_apart("AAA0BBB,M2.5.0/24,M4.1.0/0")]
    #[case::limits("AAA24BBB-24:59:59,M2.1.0/0,M11.5.6/24:00")]
    fn a_time_zone_that_both_libraries_read_alike_is_accepted(#[case] zone: &str) {
        let text = format!("{BY_NAME}\n[environment]\nTZ = \"{zone}\"\n");

        assert_matches!(
            text.parse::<Scenario>(),
            Ok(scenario) if scenario.environment == BTreeMap::from([("TZ".to_owned(), zone.to_owned())])
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
