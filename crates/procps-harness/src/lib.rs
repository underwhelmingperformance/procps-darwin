// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs procps-ng reference scenarios and compares the output of the tools
//! with golden files.
//!
//! A scenario starts fixture processes with known names in a new session, runs
//! one command in that session, and records its exit status, standard output
//! and standard error, and what happened to each fixture. The pids differ on
//! every run, so the harness replaces each one with its name in braces before
//! the output is compared or stored.

mod credentials;
mod error;
mod fixture;
mod host;
mod outcome;
mod pids;
mod runner;
mod scenario;
mod session;
mod signal;
mod spawn;

pub use credentials::Credentials;
pub use error::Error;
pub use host::host;
pub use outcome::{FixtureEnd, Outcome, Stream};
pub use runner::{Generation, Runner};
pub use scenario::{Scenario, ScenarioFile};
