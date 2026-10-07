// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks that `REUSE.toml` gives the licence of procps-ng's `src/pgrep.c` to
//! each golden file that contains procps-ng's usage text, and to no other file.

use std::{collections::BTreeSet, fs, io, path::Path};

use pretty_assertions::assert_eq;
use serde::Deserialize;

/// The licence of procps-ng's `src/pgrep.c`, which contains the usage text.
const UPSTREAM_LICENCE: &str = "GPL-2.0-or-later";

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

/// The golden files that contain procps-ng's usage text, relative to `root`.
fn goldens_with_usage(root: &Path) -> Result<BTreeSet<String>, Error> {
    let mut paths = BTreeSet::new();

    for tool in fs::read_dir(root.join("crates/procps-harness/golden"))? {
        for golden in fs::read_dir(tool?.path())? {
            let path = golden?.path();
            let contents = fs::read_to_string(&path)?;

            if contents.lines().any(|line| line == "Usage:") {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                paths.insert(relative.to_string_lossy().into_owned());
            }
        }
    }

    Ok(paths)
}

#[test]
fn the_goldens_with_procps_ng_s_usage_text_have_its_licence() -> Result<(), Error> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let reuse: Reuse = toml::from_str(&fs::read_to_string(root.join("REUSE.toml"))?)?;

    let licensed: BTreeSet<String> = reuse
        .annotations
        .into_iter()
        .filter(|annotation| annotation.licence == UPSTREAM_LICENCE)
        .flat_map(|annotation| annotation.path.into_vec())
        .collect();

    assert_eq!(licensed, goldens_with_usage(&root)?);

    Ok(())
}
