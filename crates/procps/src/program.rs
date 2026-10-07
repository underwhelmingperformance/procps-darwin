// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
};

/// The tool's invocation name, computed as glibc computes
/// `program_invocation_short_name`: the bytes of `argv[0]` after its last
/// `/`. `fallback` is the name to use when `arguments` is empty.
///
/// ```
/// let arguments = ["/usr/local/bin/pgrep", "-c"].map(std::ffi::OsString::from);
///
/// assert_eq!(procps::program_name(&arguments, "pgrep"), "pgrep");
/// ```
#[must_use]
pub fn program_name(arguments: &[OsString], fallback: &str) -> OsString {
    let Some(first) = arguments.first() else {
        return OsString::from(fallback);
    };

    let bytes = first.as_bytes();
    let start = bytes
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);

    OsStr::from_bytes(&bytes[start..]).to_owned()
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        os::unix::ffi::OsStrExt,
    };

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::program_name;

    #[rstest]
    #[case::path(b"/usr/bin/pgrep", b"pgrep")]
    #[case::bare(b"pkill", b"pkill")]
    #[case::trailing_slash(b"pgrep/", b"")]
    #[case::parent(b"..", b"..")]
    #[case::dot(b"a/.", b".")]
    #[case::raw_bytes(b"p\xffgrep", b"p\xffgrep")]
    fn the_name_is_what_follows_the_last_slash(#[case] argument: &[u8], #[case] expected: &[u8]) {
        let arguments = [OsStr::from_bytes(argument).to_owned()];

        assert_eq!(
            program_name(&arguments, "pgrep"),
            OsStr::from_bytes(expected)
        );
    }

    #[test]
    fn without_arguments_the_name_is_the_fallback() {
        assert_eq!(program_name(&[], "pkill"), OsString::from("pkill"));
    }
}
