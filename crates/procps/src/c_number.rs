// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

/// The result of C's `strtol` with base 10.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Strtol {
    /// The value, which `strtol` limits to the range of a `long` on overflow.
    pub(crate) value: i64,
    /// The number of bytes that `strtol` read, up to `endptr`.
    pub(crate) length: usize,
    /// Whether the number overflowed, which `strtol` reports with `ERANGE`.
    pub(crate) overflowed: bool,
}

/// Whether `byte` is white space to C's `isspace` in the C locale.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Reads `text` as C's `strtol(text, &endptr, 10)` does: white space, an
/// optional sign and decimal digits. Returns `None` if no digits follow the
/// white space and the sign, in which case `strtol` sets `endptr` to `text`.
pub(crate) fn strtol(text: &[u8]) -> Option<Strtol> {
    let start = text.iter().take_while(|&&byte| is_space(byte)).count();
    let (negative, digits_start) = match text.get(start) {
        Some(b'-') => (true, start + 1),
        Some(b'+') => (false, start + 1),
        _ => (false, start),
    };
    let digits = text[digits_start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();

    if digits == 0 {
        return None;
    }

    // A negative number subtracts each digit, because `i64::MIN` has no
    // positive counterpart and negating an accumulated magnitude would
    // overflow.
    let mut value: i64 = 0;
    let mut overflowed = false;

    for &byte in &text[digits_start..digits_start + digits] {
        let digit = i64::from(byte - b'0');
        let next = value.checked_mul(10).and_then(|value| {
            if negative {
                value.checked_sub(digit)
            } else {
                value.checked_add(digit)
            }
        });

        let Some(next) = next else {
            overflowed = true;
            value = if negative { i64::MIN } else { i64::MAX };
            break;
        };

        value = next;
    }

    Some(Strtol {
        value,
        length: digits_start + digits,
        overflowed,
    })
}

/// The value of C's `atoi`, which glibc computes as `(int) strtol(text, NULL,
/// 10)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "glibc's `atoi` truncates the `long` from `strtol` to an `int`"
)]
pub(crate) fn atoi(text: &[u8]) -> libc::c_int {
    strtol(text).map_or(0, |number| number.value as libc::c_int)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Strtol, atoi, strtol};

    #[rstest]
    #[case::digits("42", Some((42, 2, false)))]
    #[case::trailing("9x", Some((9, 1, false)))]
    #[case::space_and_sign(" \x0b-7", Some((-7, 4, false)))]
    #[case::plus("+0", Some((0, 2, false)))]
    #[case::sign_only("-", None)]
    #[case::empty("", None)]
    #[case::letters("x1", None)]
    #[case::too_large("9223372036854775808", Some((i64::MAX, 19, true)))]
    #[case::too_small("-9223372036854775809", Some((i64::MIN, 20, true)))]
    #[case::smallest("-9223372036854775808", Some((i64::MIN, 20, false)))]
    fn strtol_reads_as_c_does(#[case] text: &str, #[case] expected: Option<(i64, usize, bool)>) {
        let expected = expected.map(|(value, length, overflowed)| Strtol {
            value,
            length,
            overflowed,
        });

        assert_eq!(strtol(text.as_bytes()), expected);
    }

    #[rstest]
    #[case::digits("5s", 5)]
    #[case::vertical_tab("\x0b5", 5)]
    #[case::none("x", 0)]
    #[case::wraps("4294967305", 9)]
    #[case::wraps_negative("2147483648", -2_147_483_648)]
    #[case::saturates("99999999999999999999", -1)]
    fn atoi_truncates_strtol_to_an_int(#[case] text: &str, #[case] expected: libc::c_int) {
        assert_eq!(atoi(text.as_bytes()), expected);
    }
}
