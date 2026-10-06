// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Looks up the names of users, groups and terminals.

use std::{ffi::OsString, os::unix::fs::MetadataExt};

use darwin_proc::{Gid, Pid, Terminal, Uid};
use pretty_assertions::assert_eq;
use rstest::rstest;

#[rstest]
#[case::root(0, Some("root"))]
#[case::nobody(4_294_967_294, Some("nobody"))]
#[case::unknown(4_000_000_000, None)]
fn a_user_is_named(#[case] uid: u32, #[case] expected: Option<&str>) -> std::io::Result<()> {
    assert_eq!(Uid::from(uid).name()?, expected.map(OsString::from));

    Ok(())
}

#[rstest]
#[case::wheel(0, Some("wheel"))]
#[case::nogroup(4_294_967_295, Some("nogroup"))]
#[case::unknown(4_000_000_000, None)]
fn a_group_is_named(#[case] gid: u32, #[case] expected: Option<&str>) -> std::io::Result<()> {
    assert_eq!(Gid::from(gid).name()?, expected.map(OsString::from));

    Ok(())
}

#[rstest]
#[case::root("root", Some(0))]
#[case::nobody("nobody", Some(4_294_967_294))]
#[case::unknown("no-such-user", None)]
#[case::interior_nul("ro\0ot", None)]
fn a_user_name_is_found(#[case] name: &str, #[case] expected: Option<u32>) -> std::io::Result<()> {
    assert_eq!(Uid::from_name(name.as_ref())?, expected.map(Uid::from));

    Ok(())
}

#[rstest]
#[case::wheel("wheel", Some(0))]
#[case::nogroup("nogroup", Some(4_294_967_295))]
#[case::unknown("no-such-group", None)]
#[case::interior_nul("wh\0eel", None)]
fn a_group_name_is_found(#[case] name: &str, #[case] expected: Option<u32>) -> std::io::Result<()> {
    assert_eq!(Gid::from_name(name.as_ref())?, expected.map(Gid::from));

    Ok(())
}

#[rstest]
#[case::null_device("/dev/null", Some("null"))]
#[case::zero_device("/dev/zero", Some("zero"))]
fn a_device_is_named(#[case] path: &str, #[case] expected: Option<&str>) -> std::io::Result<()> {
    let terminal = Terminal {
        device: libc::dev_t::try_from(std::fs::metadata(path)?.rdev())
            .map_err(std::io::Error::other)?,
        foreground_group: Pid::from(1),
    };

    assert_eq!(terminal.name(), expected.map(OsString::from));

    Ok(())
}

#[test]
fn an_unknown_device_has_no_name() {
    let terminal = Terminal {
        device: 0x7fff_ffff,
        foreground_group: Pid::from(1),
    };

    assert_eq!(terminal.name(), None);
}
