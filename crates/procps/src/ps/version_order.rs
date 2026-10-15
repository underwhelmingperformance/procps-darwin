// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 1997-2026 Free Software Foundation, Inc.
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from glibc 2.44 string/strverscmp.c.

use std::cmp::Ordering;

/// The states of the comparison: ordinary text, an integral part, a
/// fractional part, and a fractional part that has only leading zeros so far.
const NORMAL: usize = 0;
const INTEGRAL: usize = 3;
const FRACTIONAL: usize = 6;
const ZEROS: usize = 9;

/// The result types: compare the bytes, or compare the lengths of the digit
/// runs first.
const COMPARE: i8 = 2;
const LENGTH: i8 = 3;

/// The next state for each state and class of byte: other, a digit from 1 to
/// 9, or 0.
const NEXT_STATE: [usize; 12] = [
    NORMAL, INTEGRAL, ZEROS, //
    NORMAL, INTEGRAL, INTEGRAL, //
    NORMAL, FRACTIONAL, FRACTIONAL, //
    NORMAL, FRACTIONAL, ZEROS,
];

/// The result for each state and pair of byte classes.
const RESULT_TYPE: [i8; 36] = [
    COMPARE, COMPARE, COMPARE, COMPARE, LENGTH, COMPARE, COMPARE, COMPARE, COMPARE, //
    COMPARE, -1, -1, 1, LENGTH, LENGTH, 1, LENGTH, LENGTH, //
    COMPARE, COMPARE, COMPARE, COMPARE, COMPARE, COMPARE, COMPARE, COMPARE, COMPARE, //
    COMPARE, 1, 1, -1, COMPARE, COMPARE, -1, COMPARE, COMPARE,
];

/// The class of `byte`: 0 for a byte that is not a digit, 1 for a digit from
/// 1 to 9, and 2 for 0.
fn class(byte: u8) -> usize {
    usize::from(byte == b'0') + usize::from(byte.is_ascii_digit())
}

/// Compares `left` and `right` in glibc's version order, as `strverscmp`
/// does. A run of digits compares by its numeric value, so that `ttys2`
/// comes before `ttys10`, and leading zeros make a run a fraction, so that
/// `000` comes before `00`. Each string ends at its first NUL.
pub(crate) fn compare(left: &[u8], right: &[u8]) -> Ordering {
    let byte = |text: &[u8], index: usize| text.get(index).copied().unwrap_or(0);
    let mut index = 0;
    let mut first = byte(left, 0);
    let mut second = byte(right, 0);
    let mut state = NORMAL + class(first);

    while first == second {
        if first == 0 {
            return Ordering::Equal;
        }

        state = NEXT_STATE[state];
        index += 1;
        first = byte(left, index);
        second = byte(right, index);
        state += class(first);
    }

    let difference = i32::from(first) - i32::from(second);

    match RESULT_TYPE[state * 3 + class(second)] {
        COMPARE => difference.cmp(&0),
        LENGTH => {
            let mut offset = index + 1;

            while byte(left, offset).is_ascii_digit() {
                if !byte(right, offset).is_ascii_digit() {
                    return Ordering::Greater;
                }

                offset += 1;
            }

            if byte(right, offset).is_ascii_digit() {
                return Ordering::Less;
            }

            difference.cmp(&0)
        }
        result => result.cmp(&0),
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::compare;

    #[rstest]
    #[case::equal("ttys001", "ttys001", Ordering::Equal)]
    #[case::text("console", "ttys001", Ordering::Less)]
    #[case::shorter_number("ttys2", "ttys10", Ordering::Less)]
    #[case::longer_number("ttys10", "ttys9", Ordering::Greater)]
    #[case::same_length("ttys003", "ttys001", Ordering::Greater)]
    #[case::fraction("1.01", "1.1", Ordering::Less)]
    #[case::leading_zeros("000", "00", Ordering::Less)]
    #[case::zeros_before_digit("009", "01", Ordering::Less)]
    #[case::prefix("ttys", "ttys1", Ordering::Less)]
    #[case::question_mark("?", "ttys1", Ordering::Less)]
    fn versions_compare_as_glibc_compares_them(
        #[case] left: &str,
        #[case] right: &str,
        #[case] expected: Ordering,
    ) {
        assert_eq!(
            (
                compare(left.as_bytes(), right.as_bytes()),
                compare(right.as_bytes(), left.as_bytes())
            ),
            (expected, expected.reverse())
        );
    }
}
