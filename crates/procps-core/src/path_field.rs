// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Serialises a path field with serde's encoding of an `OsString`, which on
//! Unix contains the path's bytes. serde serialises a `PathBuf` as a string,
//! which fails for a path that is not valid UTF-8.

use std::{ffi::OsString, path::PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Field;

pub(crate) fn serialize<S: Serializer>(
    path: &Field<PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    path.as_ref()
        .map(|path| path.as_os_str())
        .serialize(serializer)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Field<PathBuf>, D::Error> {
    Field::<OsString>::deserialize(deserializer).map(|path| path.map(PathBuf::from))
}

/// The same encoding for an optional path field, which is `None` when the
/// request did not ask for its field group.
pub(crate) mod optional {
    use std::{ffi::OsString, path::PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::Field;

    #[expect(
        clippy::ref_option,
        reason = "serde's `with` attribute passes a reference to the field"
    )]
    pub(crate) fn serialize<S: Serializer>(
        path: &Option<Field<PathBuf>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_ref()
            .map(|path| path.as_ref().map(|path| path.as_os_str()))
            .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Field<PathBuf>>, D::Error> {
        Option::<Field<OsString>>::deserialize(deserializer)
            .map(|path| path.map(|path| path.map(PathBuf::from)))
    }
}
