// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2009-2026 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2013-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2011-2012 Sami Kerola <kerolasa@iki.fi>
// SPDX-FileCopyrightText: 2012 Roberto Polli <rpolli@babel.it>
// SPDX-FileCopyrightText: 2002-2007 Albert Cahalan
// SPDX-FileCopyrightText: 2000 Kjetil Torgrim Homme <kjetilho@ifi.uio.no>
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 src/pgrep.c.

/// Whether a shell reads `token` as one word without quotes, as procps-ng's
/// `is_token_safe` decides: it is not empty, and it has only ASCII letters and
/// digits and the characters `_@%+:,./-`.
fn is_token_safe(token: &[u8]) -> bool {
    !token.is_empty()
        && token
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+:,./-".contains(byte))
}

/// `tokens` quoted for a POSIX shell and joined with spaces, as procps-ng's
/// `shell_quote_vector` quotes them. A token that needs quotes goes in single
/// quotes, and each single quote in it becomes `'\''`. procps-ng quotes bytes,
/// so a token that is not valid in the locale's encoding keeps its bytes.
pub(crate) fn shell_quote<'a>(tokens: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let quoted: Vec<Vec<u8>> = tokens
        .into_iter()
        .map(|token| {
            if is_token_safe(token) {
                return token.to_vec();
            }

            let mut quoted = vec![b'\''];

            for &byte in token {
                if byte == b'\'' {
                    quoted.extend_from_slice(br"'\''");
                } else {
                    quoted.push(byte);
                }
            }

            quoted.push(b'\'');
            quoted
        })
        .collect();

    quoted.join(&b' ')
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::shell_quote;

    #[rstest]
    #[case::safe(&["/usr/bin/sleep", "10"], "/usr/bin/sleep 10")]
    #[case::space(&["sh", "-c", "echo hi"], "sh -c 'echo hi'")]
    #[case::quote(&["it's"], r"'it'\''s'")]
    #[case::empty(&["a", ""], "a ''")]
    #[case::punctuation(&["a_b@c%d+e:f,g.h/i-j"], "a_b@c%d+e:f,g.h/i-j")]
    #[case::outside_ascii(&["caf\u{e9}"], "'caf\u{e9}'")]
    #[case::equals_sign(&["a=b"], "'a=b'")]
    #[case::dollar(&["$x"], "'$x'")]
    fn tokens_are_quoted_as_procps_ng_quotes_them(#[case] tokens: &[&str], #[case] expected: &str) {
        assert_eq!(
            shell_quote(tokens.iter().map(|token| token.as_bytes())),
            expected.as_bytes()
        );
    }

    #[test]
    fn a_token_that_is_not_valid_utf8_keeps_its_bytes() {
        assert_eq!(shell_quote([b"caf\xe9 x".as_slice()]), b"'caf\xe9 x'");
    }
}
