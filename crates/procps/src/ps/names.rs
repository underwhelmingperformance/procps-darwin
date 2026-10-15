// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2015-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2002 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 library/pwcache.c.

use std::{collections::HashMap, ffi::OsString, os::unix::ffi::OsStringExt};

use crate::ps::system::System;

/// The size of procps-ng's buffer for a user or group name. A longer name is
/// shown as its ID.
const NAME_SIZE: usize = 33;

/// User and group names, looked up once for each ID, as procps-ng's
/// `pwcache_get_user` and `pwcache_get_group` cache them. The rows after the
/// first with an ID need no lookup.
pub(crate) struct Names<'a> {
    system: &'a dyn System,
    users: HashMap<u32, Vec<u8>>,
    groups: HashMap<u32, Vec<u8>>,
}

impl<'a> Names<'a> {
    /// An empty cache, which asks `system` for each name.
    pub(crate) fn new(system: &'a dyn System) -> Self {
        Self {
            system,
            users: HashMap::new(),
            groups: HashMap::new(),
        }
    }

    /// The name of the user `uid`, or the ID in decimal if the user has no
    /// name, an empty name, or a name of 33 bytes or more.
    pub(crate) fn user(&mut self, uid: u32) -> &[u8] {
        let system = self.system;

        self.users
            .entry(uid)
            .or_insert_with(|| name_or_id(system.user_name(uid), uid))
    }

    /// The name of the group `gid`, or the ID in decimal if the group has no
    /// name, an empty name, or a name of 33 bytes or more.
    pub(crate) fn group(&mut self, gid: u32) -> &[u8] {
        let system = self.system;

        self.groups
            .entry(gid)
            .or_insert_with(|| name_or_id(system.group_name(gid), gid))
    }
}

/// `name`, or `id` in decimal if `name` is missing, empty or too long.
fn name_or_id(name: Option<OsString>, id: u32) -> Vec<u8> {
    match name.map(OsString::into_vec) {
        Some(name) if !name.is_empty() && name.len() < NAME_SIZE => name,
        _ => id.to_string().into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::Names;
    use crate::ps::system::testing::FakeSystem;

    /// The longest name that procps-ng keeps: 32 bytes.
    const LONGEST: &str = "abcdefghijklmnopqrstuvwxyz012345";

    /// A name of 33 bytes, one more than procps-ng keeps.
    const TOO_LONG: &str = "abcdefghijklmnopqrstuvwxyz0123456";

    #[rstest]
    #[case::known(0, "root")]
    #[case::unknown(12_345, "12345")]
    #[case::empty(400, "400")]
    #[case::too_long(401, "401")]
    #[case::longest(402, LONGEST)]
    fn a_user_is_shown_by_name_or_by_id(#[case] uid: u32, #[case] expected: &str) {
        let mut system = FakeSystem::default();
        system
            .users
            .extend([("", 400), (TOO_LONG, 401), (LONGEST, 402)]);
        let mut names = Names::new(&system);

        assert_eq!(names.user(uid), expected.as_bytes());
    }

    #[test]
    fn a_group_is_shown_by_name_or_by_id() {
        let system = FakeSystem::default();
        let mut names = Names::new(&system);

        assert_eq!(
            (names.group(20).to_vec(), names.group(30).to_vec()),
            (b"staff".to_vec(), b"30".to_vec())
        );
    }
}
