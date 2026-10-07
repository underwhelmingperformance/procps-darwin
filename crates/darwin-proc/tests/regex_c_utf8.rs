// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles regular expressions in C.UTF-8, with `LC_COLLATE` locales with and
//! without glibc's collation rules.

use std::{
    ffi::{CStr, OsStr},
    os::unix::ffi::OsStrExt,
};

use darwin_proc::{Regex, RegexErrorKind};
use pretty_assertions::assert_eq;
use rstest::rstest;

/// The texts that each expression is matched against.
const TEXTS: [&str; 3] = ["cafe", "caf\u{e9}", "caf\u{f0}"];

type Outcome = Result<[bool; 3], (RegexErrorKind, String)>;

fn collation() -> Outcome {
    Err((
        RegexErrorKind::Collation,
        "Invalid collation character".to_owned(),
    ))
}

// A name such as `C/C.UTF-8/C/C/C/C` sets `LC_COLLATE` to its first part and
// `LC_CTYPE` to its second.
#[rstest]
#[case::range_outside_ascii(c"C.UTF-8", "^caf[à-ü]$".as_bytes(), collation())]
#[case::range_from_ascii(c"C.UTF-8", "^caf[a-ü]$".as_bytes(), collation())]
#[case::range_to_ascii(c"C.UTF-8", "^caf[à-z]$".as_bytes(), collation())]
#[case::ascii_range(c"C.UTF-8", "^caf[a-z]$".as_bytes(), Ok([true, false, false]))]
#[case::byte_range(c"C.UTF-8", b"^caf[\xe9-\xef]$", Ok([false, true, false]))]
#[case::c_collation(c"C/C.UTF-8/C/C/C/C", "^caf[à-ü]$".as_bytes(), collation())]
#[case::posix_collation(c"POSIX/C.UTF-8/C/C/C/C", "^caf[à-ü]$".as_bytes(), collation())]
#[case::english_collation(
    c"en_US.UTF-8/C.UTF-8/C/C/C/C",
    "^caf[à-ü]$".as_bytes(),
    Ok([false, true, true])
)]
fn a_range_follows_the_rules_of_the_collation_locale(
    #[case] locale: &CStr,
    #[case] pattern: &[u8],
    #[case] expected: Outcome,
) {
    assert!(
        darwin_proc::set_locale(locale).is_some(),
        "{locale:?} exists"
    );

    let outcome = Regex::new(OsStr::from_bytes(pattern), false)
        .map(|regex| TEXTS.map(|text| regex.is_match(text.as_ref())))
        .map_err(|error| (error.kind, error.message));

    assert_eq!(outcome, expected);
}
