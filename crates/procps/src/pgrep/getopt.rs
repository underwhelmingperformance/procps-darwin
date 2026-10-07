// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt,
};

use crate::pgrep::{
    Mode,
    table::{self, OPTIONS, Spec, Takes},
};

/// The first error that glibc's `getopt_long` finds in a command line.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct GetoptError {
    /// The arguments before the error. When the error is in a cluster of
    /// short options, the last argument contains the options of the cluster
    /// that precede the error. procps-ng handles every option in `before`
    /// before `getopt_long` reports the error.
    pub(crate) before: Vec<OsString>,
    /// glibc's message, without the program name.
    pub(crate) message: Vec<u8>,
}

/// Where `getopt_long` stops reading options.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Stop {
    /// At an error.
    Error(GetoptError),
    /// At the first operand, which has this index in the arguments.
    /// `getopt_long` stops there when `POSIXLY_CORRECT` is set.
    Operand(usize),
    /// At `--` or at the end of the arguments.
    End,
}

/// Reads `arguments`, which start after the program name, as glibc's
/// `getopt_long` reads them for the tool that `mode` selects.
///
/// `getopt_long` normally moves the operands to the end and reads every
/// option. With `require_order`, which `POSIXLY_CORRECT` sets, it stops at
/// the first operand.
///
/// `clap` reports that an error exists, but not in glibc's words or with the
/// detail that glibc gives, such as the possibilities for an ambiguous
/// abbreviation. This scanner reads the arguments as glibc does, to find the
/// same error.
pub(crate) fn scan(mode: Mode, arguments: &[OsString], require_order: bool) -> Stop {
    let mut index = 0;
    let error = |end: usize, message: Vec<u8>| {
        Stop::Error(GetoptError {
            before: arguments[..end].to_vec(),
            message,
        })
    };

    while let Some(argument) = arguments.get(index) {
        let start = index;
        let argument = argument.as_bytes();
        index += 1;

        if argument == b"--" {
            return Stop::End;
        }

        if let Some(long) = argument.strip_prefix(b"--") {
            let (spec, value) = match long_option(long) {
                Ok(found) => found,
                Err(message) => return error(start, message),
            };
            let name = spec.long.unwrap_or_default().as_bytes();

            match (spec.takes(), value) {
                (Takes::Nothing, Some(_)) => {
                    return error(
                        start,
                        [b"option '--", name, b"' doesn't allow an argument"].concat(),
                    );
                }
                (Takes::Argument, None) if index == arguments.len() => {
                    return error(
                        start,
                        [b"option '--", name, b"' requires an argument"].concat(),
                    );
                }
                (Takes::Argument, None) => index += 1,
                (Takes::Argument | Takes::Nothing, _) => {}
            }

            continue;
        }

        let Some(cluster) = argument.strip_prefix(b"-").filter(|rest| !rest.is_empty()) else {
            if require_order {
                return Stop::Operand(start);
            }

            continue;
        };

        let cluster_error = |position: usize, message: Vec<u8>| {
            let mut found = GetoptError {
                before: arguments[..start].to_vec(),
                message,
            };

            if position > 0 {
                let options = [b"-", &cluster[..position]].concat();
                found.before.push(OsStr::from_bytes(&options).to_owned());
            }

            Stop::Error(found)
        };

        for (position, &option) in cluster.iter().enumerate() {
            match table::by_short(mode, option).map(Spec::takes) {
                None => {
                    return cluster_error(
                        position,
                        [b"invalid option -- '", &[option][..], b"'"].concat(),
                    );
                }
                Some(Takes::Nothing) => {}
                Some(Takes::Argument) if position + 1 < cluster.len() => break,
                Some(Takes::Argument) if index == arguments.len() => {
                    return cluster_error(
                        position,
                        [b"option requires an argument -- '", &[option][..], b"'"].concat(),
                    );
                }
                Some(Takes::Argument) => {
                    index += 1;
                    break;
                }
            }
        }
    }

    Stop::End
}

/// `arguments`, which start after the program name, with some values moved
/// into arguments of their own. A value moves when it is attached to a short
/// option and starts with `=`. `getopt_long` reads `-d=x` as `-d` with the
/// value `=x`, but `clap` drops the `=`. `clap` keeps a value unchanged when
/// the value is a separate argument, as in `-d` `=x`.
pub(crate) fn detach_equals_values(mode: Mode, arguments: &[OsString]) -> Vec<OsString> {
    let mut detached = Vec::with_capacity(arguments.len());
    let mut rest = arguments.iter();

    while let Some(argument) = rest.next() {
        let bytes = argument.as_bytes();

        if bytes == b"--" {
            detached.push(argument.clone());
            detached.extend(rest.cloned());
            break;
        }

        let takes_next = if let Some(long) = bytes.strip_prefix(b"--") {
            matches!(long_option(long), Ok((spec, None)) if spec.takes() == Takes::Argument)
        } else if let Some(cluster) = bytes.strip_prefix(b"-").filter(|rest| !rest.is_empty()) {
            let value_at = cluster.iter().position(|&option| {
                table::by_short(mode, option).is_some_and(|spec| spec.takes() == Takes::Argument)
            });

            match value_at {
                Some(position) if cluster.get(position + 1) == Some(&b'=') => {
                    let (options, value) = cluster.split_at(position + 1);
                    let options = [b"-", options].concat();

                    detached.push(OsStr::from_bytes(&options).to_owned());
                    detached.push(OsStr::from_bytes(value).to_owned());
                    continue;
                }
                Some(position) => position + 1 == cluster.len(),
                None => false,
            }
        } else {
            false
        };

        detached.push(argument.clone());

        if takes_next && let Some(value) = rest.next() {
            detached.push(value.clone());
        }
    }

    detached
}

/// The long option that `long` selects, and its value after `=`. `long` is
/// the argument after `--`. Returns glibc's message if `long` selects no
/// single option.
fn long_option(long: &[u8]) -> Result<(&'static Spec, Option<&[u8]>), Vec<u8>> {
    let (name, value) = match long.iter().position(|&byte| byte == b'=') {
        Some(equals) => (&long[..equals], Some(&long[equals + 1..])),
        None => (long, None),
    };
    let named = |spec: &&Spec| spec.long.is_some_and(|known| known.as_bytes() == name);

    if let Some(spec) = OPTIONS.iter().find(named) {
        return Ok((spec, value));
    }

    let candidates: Vec<&Spec> = OPTIONS
        .iter()
        .filter(|spec| {
            spec.long
                .is_some_and(|known| known.as_bytes().starts_with(name))
        })
        .collect();

    match candidates.as_slice() {
        [only] => Ok((only, value)),
        [] => Err([b"unrecognized option '--", long, b"'"].concat()),
        candidates => {
            let mut message = [b"option '--", long, b"' is ambiguous; possibilities:"].concat();

            for known in candidates.iter().filter_map(|spec| spec.long) {
                message.extend_from_slice(b" '--");
                message.extend_from_slice(known.as_bytes());
                message.push(b'\'');
            }

            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        os::unix::ffi::OsStrExt,
    };

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{GetoptError, Stop, scan};
    use crate::pgrep::Mode;

    fn arguments(arguments: &[&str]) -> Vec<OsString> {
        arguments.iter().map(OsString::from).collect()
    }

    /// The error with `message`, after the arguments `before`.
    fn error(before: &[&str], message: &[u8]) -> Stop {
        Stop::Error(GetoptError {
            before: arguments(before),
            message: message.to_vec(),
        })
    }

    #[rstest]
    #[case::invalid_short(Mode::Pgrep, &["-z"], error(&[], b"invalid option -- 'z'"))]
    #[case::other_mode(Mode::Pkill, &["-l"], error(&[], b"invalid option -- 'l'"))]
    #[case::colon(Mode::Pgrep, &["-c:"], error(&["-c"], b"invalid option -- ':'"))]
    #[case::semicolon(Mode::Pgrep, &["-;"], error(&[], b"invalid option -- ';'"))]
    #[case::missing_short(Mode::Pgrep, &["-d"], error(&[], b"option requires an argument -- 'd'"))]
    #[case::clustered(Mode::Pgrep, &["-cd"], error(&["-c"], b"option requires an argument -- 'd'"))]
    #[case::attached(Mode::Pgrep, &["-dx", "-z"], error(&["-dx"], b"invalid option -- 'z'"))]
    #[case::separate(Mode::Pgrep, &["-d", "-z"], Stop::End)]
    #[case::unrecognized(Mode::Pgrep, &["--bogus=1"], error(&[], b"unrecognized option '--bogus=1'"))]
    #[case::ambiguous(
        Mode::Pgrep,
        &["--pi", "1"],
        error(&[], b"option '--pi' is ambiguous; possibilities: '--pid' '--pidfile'")
    )]
    #[case::ambiguous_with_value(
        Mode::Pgrep,
        &["--pi=1"],
        error(&[], b"option '--pi=1' is ambiguous; possibilities: '--pid' '--pidfile'")
    )]
    #[case::exact(Mode::Pgrep, &["--pid", "1"], Stop::End)]
    #[case::unwanted(Mode::Pgrep, &["--cou=1"], error(&[], b"option '--count' doesn't allow an argument"))]
    #[case::missing_long(Mode::Pgrep, &["--pid"], error(&[], b"option '--pid' requires an argument"))]
    #[case::after_separator(Mode::Pgrep, &["--", "-z"], Stop::End)]
    #[case::pattern(Mode::Pgrep, &["-", "x"], Stop::End)]
    fn the_first_error_is_glibc_s(
        #[case] mode: Mode,
        #[case] given: &[&str],
        #[case] expected: Stop,
    ) {
        assert_eq!(scan(mode, &arguments(given), false), expected);
    }

    #[rstest]
    #[case::multibyte_option(b"-\xc3\xa9", b"invalid option -- '\xc3'")]
    #[case::long_option(b"--\xff", b"unrecognized option '--\xff'")]
    fn a_message_has_the_argument_s_bytes(#[case] argument: &[u8], #[case] expected: &[u8]) {
        let arguments = [OsStr::from_bytes(argument).to_owned()];

        assert_eq!(scan(Mode::Pgrep, &arguments, false), error(&[], expected));
    }

    #[rstest]
    #[case::separate(&["-c", "x", "-z"], &["-c", "x"], "invalid option -- 'z'")]
    #[case::in_cluster(&["-c", "-iz"], &["-c", "-i"], "invalid option -- 'z'")]
    #[case::cluster_start(&["-zi"], &[], "invalid option -- 'z'")]
    #[case::missing_in_cluster(&["-cd"], &["-c"], "option requires an argument -- 'd'")]
    #[case::long(&["-c", "--bogus"], &["-c"], "unrecognized option '--bogus'")]
    fn the_error_keeps_the_options_before_it(
        #[case] given: &[&str],
        #[case] before: &[&str],
        #[case] message: &str,
    ) {
        let expected = Stop::Error(GetoptError {
            before: arguments(before),
            message: message.as_bytes().to_vec(),
        });

        assert_eq!(scan(Mode::Pgrep, &arguments(given), false), expected);
    }

    #[rstest]
    #[case::operand(&["-c", "x", "-z"], Stop::Operand(1))]
    #[case::option_value(&["-d", "x", "y"], Stop::Operand(2))]
    #[case::dash(&["-", "-c"], Stop::Operand(0))]
    #[case::separator(&["--", "x"], Stop::End)]
    fn posixly_correct_stops_at_the_first_operand(#[case] given: &[&str], #[case] expected: Stop) {
        assert_eq!(scan(Mode::Pgrep, &arguments(given), true), expected);
    }
}
