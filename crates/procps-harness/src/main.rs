// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! The procps-ng reference harness.
//!
//! `procps-harness generate` runs every scenario against procps-ng and writes
//! the golden files. The tests of the `procps` package run the same scenarios
//! against this project's tools and compare the results with those files.

use std::{io::Write, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use procps_harness::{Credentials, Runner, ScenarioFile};

#[derive(Debug, Parser)]
#[command(version, about = "Runs the procps-ng reference scenarios")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Runs every scenario against procps-ng and writes the golden files.
    Generate {
        /// The directory that contains procps-ng's `pgrep`, `ps` and other
        /// tools.
        #[arg(long)]
        tools: PathBuf,
        /// The directory of scenarios, with one subdirectory for each tool.
        #[arg(long, default_value = "crates/procps-harness/scenarios")]
        scenarios: PathBuf,
        /// The directory to write the golden files to.
        #[arg(long, default_value = "crates/procps-harness/golden")]
        golden: PathBuf,
        /// The user and group, as `<uid>:<gid>`, to run the scenarios'
        /// processes as. The harness must run as root to use this.
        #[arg(long)]
        user: Option<Credentials>,
    },
}

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error(transparent)]
    Harness(#[from] procps_harness::Error),
    #[error("cannot find this program's executable: {0}")]
    CurrentExe(std::io::Error),
    #[error("cannot write to standard error: {0}")]
    Report(std::io::Error),
}

fn main() -> ExitCode {
    procps_harness::host(|| match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "procps-harness: {error}");
            ExitCode::FAILURE
        }
    })
}

fn run(cli: Cli) -> Result<(), MainError> {
    match cli.command {
        Command::Generate {
            tools,
            scenarios,
            golden,
            user,
        } => {
            let host = std::env::current_exe().map_err(MainError::CurrentExe)?;
            let runner = Runner::new(tools, host);
            let runner = match user {
                Some(credentials) => runner.with_user(credentials),
                None => runner,
            };
            runner.require_reference()?;

            let generation = runner.generate(&ScenarioFile::discover(&scenarios)?, &golden)?;
            let mut stderr = std::io::stderr().lock();

            for path in generation.written {
                writeln!(stderr, "wrote {}", path.display()).map_err(MainError::Report)?;
            }

            for path in generation.removed {
                writeln!(stderr, "removed {}", path.display()).map_err(MainError::Report)?;
            }

            Ok(())
        }
    }
}
