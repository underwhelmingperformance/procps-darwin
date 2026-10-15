// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2015-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2024 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2011 Lukas Nykryn <lnykryn@redhat.com>
// SPDX-FileCopyrightText: 1999-2004 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/ps/output.c.

use std::cmp::Ordering;

use crate::ps::glibc_ctype::{PRINTABLE, WIDTHS};

/// Text that fits in a number of screen cells, as procps-ng's `escape_str`
/// copies it into a column.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Escaped {
    /// The bytes.
    pub(crate) bytes: Vec<u8>,
    /// The screen cells that the bytes take.
    pub(crate) cells: usize,
}

/// `text` as procps-ng's `escape_str` copies it for `ps`, into at most
/// `max_cells` screen cells and `buffer - 1` bytes. `text` ends at its first
/// NUL.
///
/// In a UTF-8 locale, a byte that does not start a valid character becomes
/// `?`, and so does a character that glibc's `iswprint` rejects, such as a
/// control character, an unassigned code point, a noncharacter, or U+2028
/// and U+2029. Each `?` takes one cell, and any other character takes the
/// cells that glibc's `wcwidth` gives it. In any other locale, procps-ng
/// assumes an 8-bit encoding: a control byte becomes `.`, a byte above 0x7f
/// becomes `?`, and each byte takes one cell.
pub(crate) fn escape(text: &[u8], utf8: bool, max_cells: usize, buffer: usize) -> Escaped {
    let end = text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len());
    let text = &text[..end];

    if buffer == 0 || max_cells == 0 {
        return Escaped::default();
    }

    if utf8 {
        return escape_utf8(text, max_cells, buffer);
    }

    let buffer = buffer.min(max_cells + 1);
    let bytes: Vec<u8> = text
        .iter()
        .take(max_cells.min(buffer - 1))
        .map(|&byte| match byte {
            0x20..=0x7e => byte,
            0x80..=0xff => b'?',
            _ => b'.',
        })
        .collect();

    Escaped {
        cells: bytes.len(),
        bytes,
    }
}

/// [`escape`] in a UTF-8 locale.
fn escape_utf8(text: &[u8], max_cells: usize, buffer: usize) -> Escaped {
    let mut escaped = Escaped::default();
    let mut rest = text;

    while escaped.cells < max_cells && escaped.bytes.len() + 1 < buffer {
        let Some(&first) = rest.first() else {
            break;
        };

        if first.is_ascii() {
            escaped
                .bytes
                .push(if first.is_ascii_graphic() || first == b' ' {
                    first
                } else {
                    b'?'
                });
            escaped.cells += 1;
            rest = &rest[1..];
            continue;
        }

        let Some(character) = first_character(rest) else {
            escaped.bytes.push(b'?');
            escaped.cells += 1;
            rest = &rest[1..];
            continue;
        };
        let length = character.len_utf8();

        let Some(width) = width(character) else {
            escaped.bytes.push(b'?');
            escaped.cells += 1;
            rest = &rest[length..];
            continue;
        };

        if width > max_cells - escaped.cells || length >= buffer - (escaped.bytes.len() + 1) {
            break;
        }

        escaped.bytes.extend_from_slice(&rest[..length]);
        escaped.cells += width;
        rest = &rest[length..];
    }

    escaped
}

/// The screen cells of `character`, as glibc's `wcwidth` gives them in a
/// UTF-8 locale, or `None` if glibc's `iswprint` rejects the character.
fn width(character: char) -> Option<usize> {
    let code = u32::from(character);
    let contains = |first: u32, last: u32| {
        if code < first {
            Ordering::Greater
        } else if code > last {
            Ordering::Less
        } else {
            Ordering::Equal
        }
    };

    PRINTABLE
        .binary_search_by(|&(first, last)| contains(first, last))
        .ok()?;

    let width = WIDTHS
        .binary_search_by(|&(first, last, _)| contains(first, last))
        .map_or(1, |index| WIDTHS[index].2);

    Some(usize::from(width))
}

/// The character that `text` starts with, or `None` if `text` does not start
/// with a valid UTF-8 sequence.
fn first_character(text: &[u8]) -> Option<char> {
    let prefix = &text[..text.len().min(4)];
    let valid = match std::str::from_utf8(prefix) {
        Ok(valid) => valid,
        Err(error) => std::str::from_utf8(&prefix[..error.valid_up_to()]).ok()?,
    };

    valid.chars().next()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Escaped, escape};

    fn escaped(bytes: &[u8], cells: usize) -> Escaped {
        Escaped {
            bytes: bytes.to_vec(),
            cells,
        }
    }

    #[rstest]
    #[case::fits(b"sleep 10", true, 27, escaped(b"sleep 10", 8))]
    #[case::cut_to_the_cells(b"sleep 10", true, 5, escaped(b"sleep", 5))]
    #[case::ends_at_nul(b"one\0two", true, 27, escaped(b"one", 3))]
    #[case::no_cells(b"one", true, 0, escaped(b"", 0))]
    #[case::control_in_utf8(b"a\tb", true, 27, escaped(b"a?b", 3))]
    #[case::wide_character("日本".as_bytes(), true, 27, escaped("日本".as_bytes(), 4))]
    #[case::wide_character_does_not_fit("日本".as_bytes(), true, 3, escaped("日".as_bytes(), 2))]
    #[case::invalid_sequence(b"a\xffb", true, 27, escaped(b"a?b", 3))]
    #[case::control_in_eight_bits(b"a\tb", false, 27, escaped(b"a.b", 3))]
    #[case::delete_in_eight_bits(b"a\x7fb", false, 27, escaped(b"a.b", 3))]
    #[case::high_byte_in_eight_bits("é".as_bytes(), false, 27, escaped(b"??", 2))]
    #[case::eight_bits_cut(b"sleep 10", false, 5, escaped(b"sleep", 5))]
    fn text_is_escaped_into_its_cells(
        #[case] text: &[u8],
        #[case] utf8: bool,
        #[case] cells: usize,
        #[case] expected: Escaped,
    ) {
        assert_eq!(escape(text, utf8, cells, 131_072), expected);
    }

    #[rstest]
    #[case::unassigned('\u{378}', escaped(b"a?b", 3))]
    #[case::noncharacter('\u{fdd0}', escaped(b"a?b", 3))]
    #[case::last_noncharacter('\u{10ffff}', escaped(b"a?b", 3))]
    #[case::line_separator('\u{2028}', escaped(b"a?b", 3))]
    #[case::paragraph_separator('\u{2029}', escaped(b"a?b", 3))]
    #[case::soft_hyphen('\u{ad}', escaped("a\u{ad}b".as_bytes(), 3))]
    #[case::arabic_number_mark('\u{605}', escaped("a\u{605}b".as_bytes(), 3))]
    #[case::combining_mark('\u{301}', escaped("a\u{301}b".as_bytes(), 2))]
    #[case::wide_tone_mark('\u{302e}', escaped("a\u{302e}b".as_bytes(), 4))]
    #[case::emoji('\u{1f600}', escaped("a\u{1f600}b".as_bytes(), 4))]
    fn characters_are_printable_and_wide_as_in_glibc(
        #[case] character: char,
        #[case] expected: Escaped,
    ) {
        let text = format!("a{character}b");

        assert_eq!(escape(text.as_bytes(), true, 27, 131_072), expected);
    }

    #[rstest]
    #[case::utf8(b"abcdef", true, escaped(b"abc", 3))]
    #[case::eight_bits(b"abcdef", false, escaped(b"abc", 3))]
    #[case::character_that_does_not_fit("ab\u{e9}".as_bytes(), true, escaped(b"ab", 2))]
    fn the_buffer_keeps_a_byte_for_the_nul(
        #[case] text: &[u8],
        #[case] utf8: bool,
        #[case] expected: Escaped,
    ) {
        assert_eq!(escape(text, utf8, 27, 4), expected);
    }
}
