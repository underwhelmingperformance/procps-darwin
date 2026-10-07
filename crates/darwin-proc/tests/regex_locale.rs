// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles regular expressions in a UTF-8 locale.

use darwin_proc::Regex;
use pretty_assertions::assert_eq;

#[test]
fn a_utf8_pattern_is_read_as_characters() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        darwin_proc::set_locale(c"en_US.UTF-8").map(|locale| locale.codeset().to_owned()),
        Some("UTF-8".to_owned())
    );

    let matches = [
        Regex::new("^caf.$".as_ref(), false)?.is_match("café".as_ref()),
        Regex::new("^CAFÉ$".as_ref(), true)?.is_match("café".as_ref()),
        Regex::new("^[[:alpha:]]+$".as_ref(), false)?.is_match("café".as_ref()),
        Regex::new("^caf[à-ü]$".as_ref(), false)?.is_match("café".as_ref()),
    ];

    assert_eq!(matches, [true, true, true, true]);

    Ok(())
}
