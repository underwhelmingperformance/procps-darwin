// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! The process model, process data sources and output formatting that `ps`,
//! `top`, `pgrep` and `pkill` share.

mod field;
mod request;
mod snapshot;
mod source;

pub use field::Field;
pub use request::{FieldGroup, SnapshotRequest};
pub use snapshot::{Process, Snapshot, System, Usage};
pub use source::{FixtureSource, LocalSource, ProcessSource, SourceError};
