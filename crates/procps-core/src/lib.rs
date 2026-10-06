// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! The process model, process data sources and output formatting that `ps`,
//! `top`, `pgrep` and `pkill` share.

mod choice;
mod field;
pub mod helper;
mod linux;
mod path_field;
mod report;
mod request;
mod snapshot;
mod source;
mod summary;

pub use choice::{ChoiceError, HelperOrLocal, SOURCE_VARIABLE, Source, SourceChoice};
pub use field::Field;
pub use linux::{LinuxPolicy, LinuxPriority, LinuxSizes, LinuxState};
pub use report::ErrorChain;
pub use request::{FieldGroup, SnapshotRequest};
pub use snapshot::{Process, RegionTotals, Snapshot, System, Usage};
pub use source::{FixtureSource, LocalSource, ProcessSource, SourceError};
pub use summary::{LinuxCpu, LinuxMemory, LinuxSwap};
