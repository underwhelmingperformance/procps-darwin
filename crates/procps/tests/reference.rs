// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs the procps-ng reference scenarios against this package's binaries and
//! compares each result with the golden file that procps-ng produced.
//!
//! Each scenario is a separate test. A scenario marked `pending` is for a tool
//! that does not yet behave as procps-ng does. Its test passes while the result
//! differs, and fails once the result matches, so that the mark is removed
//! when the tool is ready.

use std::{io::Write, path::Path, process::ExitCode, sync::Arc};

use libtest_mimic::{Arguments, Failed, Trial};
use pretty_assertions::Comparison;
use procps_harness::{Outcome, Runner, ScenarioFile};

#[derive(Debug, thiserror::Error)]
enum ReferenceError {
    #[error(transparent)]
    Harness(#[from] procps_harness::Error),
    #[error("cannot find this test's executable: {0}")]
    CurrentExe(std::io::Error),
}

fn main() -> ExitCode {
    procps_harness::host(|| {
        let arguments = Arguments::from_args();

        match trials() {
            Ok(trials) => libtest_mimic::run(&arguments, trials).exit_code(),
            Err(error) => {
                let _ = writeln!(std::io::stderr().lock(), "reference: {error}");
                ExitCode::FAILURE
            }
        }
    })
}

fn trials() -> Result<Vec<Trial>, ReferenceError> {
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("../procps-harness");
    let tools = Path::new(env!("CARGO_BIN_EXE_pgrep"))
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let host = std::env::current_exe().map_err(ReferenceError::CurrentExe)?;
    let runner = Arc::new(Runner::new(tools, host));
    let golden = harness.join("golden");

    let trials = ScenarioFile::discover(&harness.join("scenarios"))?
        .into_iter()
        .map(|file| {
            let runner = Arc::clone(&runner);
            let path = file.golden_path(&golden);
            let kind = if file.scenario.is_pending() {
                "pending"
            } else {
                ""
            };

            Trial::test(format!("{}/{}", file.tool, file.name), move || {
                compare(&runner, &file, &path)
            })
            .with_kind(kind)
        })
        .collect();

    Ok(trials)
}

fn compare(runner: &Runner, file: &ScenarioFile, golden: &Path) -> Result<(), Failed> {
    let expected = Outcome::read(golden)?;
    let actual = runner.run(&file.scenario)?;

    match (file.scenario.is_pending(), expected == actual) {
        (false, true) | (true, false) => Ok(()),
        (false, false) => Err(Comparison::new(&expected, &actual).into()),
        (true, true) => {
            Err("the result matches procps-ng, so remove `pending` from the scenario".into())
        }
    }
}
