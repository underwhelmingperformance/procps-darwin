// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

/// The base that C's `strtol` and `strtoul` read a number in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Base {
    /// Base 10.
    Decimal,
    /// Base 0: hexadecimal after `0x` or `0X`, binary after `0b` or `0B`, octal
    /// after another leading `0`, and decimal otherwise. glibc reads the
    /// binary prefix since version 2.38, as C23 requires, and procps-ng uses
    /// those functions because it defines `_GNU_SOURCE`.
    Prefixed,
}

/// The result of C's `strtol`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Strtol {
    /// The value, which `strtol` limits to the range of a `long` on overflow.
    pub(crate) value: i64,
    /// The number of bytes that `strtol` read, up to `endptr`.
    pub(crate) length: usize,
    /// Whether the number overflowed, which `strtol` reports with `ERANGE`.
    pub(crate) overflowed: bool,
}

/// The result of C's `strtoul`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Strtoul {
    /// The value. `strtoul` negates the value of a number with a minus sign in
    /// unsigned arithmetic, and returns `ULONG_MAX` on overflow.
    pub(crate) value: u64,
    /// The number of bytes that `strtoul` read, up to `endptr`.
    pub(crate) length: usize,
}

/// The digits of a number, as `strtol` and `strtoul` both read them.
struct Digits {
    /// Whether a minus sign comes before the digits.
    negative: bool,
    /// The value of the digits, or `None` if it does not fit in 64 bits.
    magnitude: Option<u64>,
    /// The number of bytes up to the end of the digits.
    length: usize,
}

/// Whether `byte` is white space to C's `isspace` in the C locale.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The value of `byte` as a digit in `radix`, if it is one.
fn digit(byte: u8, radix: u32) -> Option<u64> {
    char::from(byte).to_digit(radix).map(u64::from)
}

/// Reads white space, an optional sign, the prefix that `base` allows and the
/// digits, as glibc's `strtol` and `strtoul` do. Returns `None` if no digits
/// follow. Both functions then set `endptr` to `text`.
fn digits(text: &[u8], base: Base) -> Option<Digits> {
    let start = text.iter().take_while(|&&byte| is_space(byte)).count();
    let (negative, after_sign) = match text.get(start) {
        Some(b'-') => (true, start + 1),
        Some(b'+') => (false, start + 1),
        _ => (false, start),
    };
    let rest = &text[after_sign..];

    // glibc reads `0x` and `0b` as prefixes only when a digit of their base
    // follows. Otherwise it reads the `0` as an octal number and stops before
    // the letter.
    let (radix, digits_start) = match (base, rest) {
        (Base::Prefixed, [b'0', b'x' | b'X', next, ..]) if next.is_ascii_hexdigit() => {
            (16, after_sign + 2)
        }
        (Base::Prefixed, [b'0', b'b' | b'B', b'0' | b'1', ..]) => (2, after_sign + 2),
        (Base::Prefixed, [b'0', ..]) => (8, after_sign),
        _ => (10, after_sign),
    };

    let count = text[digits_start..]
        .iter()
        .take_while(|&&byte| digit(byte, radix).is_some())
        .count();

    if count == 0 {
        return None;
    }

    let magnitude = text[digits_start..digits_start + count]
        .iter()
        .filter_map(|&byte| digit(byte, radix))
        .try_fold(0_u64, |value, digit| {
            value
                .checked_mul(u64::from(radix))
                .and_then(|value| value.checked_add(digit))
        });

    Some(Digits {
        negative,
        magnitude,
        length: digits_start + count,
    })
}

/// Reads `text` as C's `strtol(text, &endptr, base)` does. Returns `None` if
/// no digits follow the white space, the sign and the prefix. `strtol` then
/// sets `endptr` to `text`.
pub(crate) fn strtol(text: &[u8], base: Base) -> Option<Strtol> {
    let digits = digits(text, base)?;
    let value = digits.magnitude.and_then(|magnitude| {
        if digits.negative {
            0_i64.checked_sub_unsigned(magnitude)
        } else {
            i64::try_from(magnitude).ok()
        }
    });
    let limit = if digits.negative { i64::MIN } else { i64::MAX };

    Some(Strtol {
        value: value.unwrap_or(limit),
        length: digits.length,
        overflowed: value.is_none(),
    })
}

/// Reads `text` as C's `strtoul(text, &endptr, base)` does. Returns `None` if
/// no digits follow the white space, the sign and the prefix. `strtoul` then
/// sets `endptr` to `text`.
pub(crate) fn strtoul(text: &[u8], base: Base) -> Option<Strtoul> {
    let digits = digits(text, base)?;
    let value = match digits.magnitude {
        Some(magnitude) if digits.negative => magnitude.wrapping_neg(),
        Some(magnitude) => magnitude,
        None => u64::MAX,
    };

    Some(Strtoul {
        value,
        length: digits.length,
    })
}

/// The value of C's `strtol(text, &endptr, base)` if the conversion reads all
/// of `text`, and otherwise `None`. An empty `text` gives `Some(0)`, as it
/// does for a C caller that checks only that `*endptr` is `'\0'`.
pub(crate) fn whole_strtol(text: &[u8], base: Base) -> Option<i64> {
    match strtol(text, base) {
        Some(number) => (number.length == text.len()).then_some(number.value),
        None => text.is_empty().then_some(0),
    }
}

/// The value of C's `strtoul(text, &endptr, base)` if the conversion reads all
/// of `text`, and otherwise `None`. An empty `text` gives `Some(0)`, as it
/// does for a C caller that checks only that `*endptr` is `'\0'`.
pub(crate) fn whole_strtoul(text: &[u8], base: Base) -> Option<u64> {
    match strtoul(text, base) {
        Some(number) => (number.length == text.len()).then_some(number.value),
        None => text.is_empty().then_some(0),
    }
}

/// The value of C's `atoi`, which glibc computes as `(int) strtol(text, NULL,
/// 10)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "glibc's `atoi` truncates the `long` from `strtol` to an `int`"
)]
pub(crate) fn atoi(text: &[u8]) -> libc::c_int {
    strtol(text, Base::Decimal).map_or(0, |number| number.value as libc::c_int)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Base, Strtol, Strtoul, atoi, strtol, strtoul, whole_strtol, whole_strtoul};

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
    #[case::beyond_64_bits("99999999999999999999", Some((i64::MAX, 20, true)))]
    #[case::leading_zero("010", Some((10, 3, false)))]
    fn strtol_reads_as_c_does(#[case] text: &str, #[case] expected: Option<(i64, usize, bool)>) {
        let expected = expected.map(|(value, length, overflowed)| Strtol {
            value,
            length,
            overflowed,
        });

        assert_eq!(strtol(text.as_bytes(), Base::Decimal), expected);
    }

    #[rstest]
    #[case::decimal("12", Some((12, 2)))]
    #[case::octal("010", Some((8, 3)))]
    #[case::not_octal("08", Some((0, 1)))]
    #[case::hexadecimal("0x1F", Some((31, 4)))]
    #[case::upper_case_prefix("0X10", Some((16, 4)))]
    #[case::prefix_without_digits("0x", Some((0, 1)))]
    #[case::prefix_before_letter("0xg", Some((0, 1)))]
    #[case::binary("0b101", Some((5, 5)))]
    #[case::upper_case_binary("0B1", Some((1, 3)))]
    #[case::binary_without_digits("0b", Some((0, 1)))]
    #[case::binary_before_other_digit("0b2", Some((0, 1)))]
    #[case::negative_binary("-0b10", Some((u64::MAX - 1, 5)))]
    #[case::zero("0", Some((0, 1)))]
    #[case::negative("-1", Some((u64::MAX, 2)))]
    #[case::negative_hexadecimal(" -0x2", Some((u64::MAX - 1, 5)))]
    #[case::largest("18446744073709551615", Some((u64::MAX, 20)))]
    #[case::too_large("18446744073709551616", Some((u64::MAX, 20)))]
    #[case::too_small("-18446744073709551616", Some((u64::MAX, 21)))]
    #[case::letters("pid", None)]
    fn strtoul_with_base_0_reads_as_c_does(
        #[case] text: &str,
        #[case] expected: Option<(u64, usize)>,
    ) {
        let expected = expected.map(|(value, length)| Strtoul { value, length });

        assert_eq!(strtoul(text.as_bytes(), Base::Prefixed), expected);
    }

    #[rstest]
    #[case::number("0x10", Some(16))]
    #[case::empty("", Some(0))]
    #[case::trailing("5x", None)]
    #[case::white_space_only(" ", None)]
    #[case::sign_only("-", None)]
    #[case::negative("-3", Some(-3))]
    fn a_whole_number_ends_the_text(#[case] text: &str, #[case] expected: Option<i64>) {
        let unsigned = expected.map(i64::cast_unsigned);

        assert_eq!(
            (
                whole_strtol(text.as_bytes(), Base::Prefixed),
                whole_strtoul(text.as_bytes(), Base::Prefixed)
            ),
            (expected, unsigned)
        );
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
