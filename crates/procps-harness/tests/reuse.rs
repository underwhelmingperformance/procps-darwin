// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks the licences that `REUSE.toml` gives the golden files with text from
//! procps-ng's source: each tool's usage text, and the listing of `ps L`. Such
//! a file has the licence of the procps-ng file that the text comes from:
//! GPL-2.0-or-later for `pgrep` and `pkill`, and LGPL-2.1-or-later for `ps`.
//! No other golden file has those licences.

use std::{collections::BTreeMap, fs, io, path::Path};

use pretty_assertions::assert_eq;
use serde::Deserialize;

/// The directory of the golden files, relative to the root of the repository.
const GOLDEN: &str = "crates/procps-harness/golden";

/// The licence of the procps-ng file that contains the usage text of `tool`:
/// `src/ps/help.c` for `ps`, and `src/pgrep.c` for `pgrep` and `pkill`.
fn usage_licence(tool: &str) -> &'static str {
    match tool {
        "ps" => "LGPL-2.1-or-later",
        _ => "GPL-2.0-or-later",
    }
}

/// The licence of `src/ps/output.c`, whose table of format specifiers `ps L`
/// lists.
const LISTING_LICENCE: &str = "LGPL-2.1-or-later";

/// The first line of the listing that `ps L` prints.
const LISTING_START: &str = "%cpu         %CPU    ";

/// Looks for usage text, or for the listing of `ps L`, in `contents`, the text
/// of a golden file of `tool`. Returns the licence of the procps-ng file that
/// the text comes from, or `None` if `contents` has neither.
fn upstream_licence(tool: &str, contents: &str) -> Option<&'static str> {
    if contents.lines().any(|line| line == "Usage:") {
        return Some(usage_licence(tool));
    }

    let listing = tool == "ps" && contents.lines().any(|line| line == LISTING_START);

    listing.then_some(LISTING_LICENCE)
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("cannot read the repository")]
    Read(#[from] io::Error),
    #[error("cannot parse REUSE.toml")]
    Parse(#[from] toml::de::Error),
}

#[derive(Deserialize)]
struct Reuse {
    annotations: Vec<Annotation>,
}

#[derive(Deserialize)]
struct Annotation {
    path: Paths,
    #[serde(rename = "SPDX-License-Identifier")]
    licence: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Paths {
    One(String),
    Many(Vec<String>),
}

impl Paths {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(path) => vec![path],
            Self::Many(paths) => paths,
        }
    }
}

/// The golden files with text from procps-ng's source, relative to `root`,
/// with the licence of the text.
fn goldens_from_upstream(root: &Path) -> Result<BTreeMap<String, String>, Error> {
    let mut paths = BTreeMap::new();

    for tool in fs::read_dir(root.join(GOLDEN))? {
        let tool = tool?;
        let name = tool.file_name().to_string_lossy().into_owned();

        for golden in fs::read_dir(tool.path())? {
            let path = golden?.path();
            let contents = fs::read_to_string(&path)?;

            if let Some(licence) = upstream_licence(&name, &contents) {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                paths.insert(relative.to_string_lossy().into_owned(), licence.to_owned());
            }
        }
    }

    Ok(paths)
}

#[test]
fn the_goldens_with_procps_ng_s_text_have_its_licence() -> Result<(), Error> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let reuse: Reuse = toml::from_str(&fs::read_to_string(root.join("REUSE.toml"))?)?;

    let upstream_licences = [usage_licence("ps"), usage_licence("pgrep"), LISTING_LICENCE];
    let licensed: BTreeMap<String, String> = reuse
        .annotations
        .into_iter()
        .filter(|annotation| upstream_licences.contains(&annotation.licence.as_str()))
        .flat_map(|annotation| {
            let licence = annotation.licence;

            annotation
                .path
                .into_vec()
                .into_iter()
                .map(move |path| (path, licence.clone()))
        })
        .filter(|(path, _)| path.starts_with(GOLDEN))
        .collect();

    assert_eq!(licensed, goldens_from_upstream(&root)?);

    Ok(())
}
