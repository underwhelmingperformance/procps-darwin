// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! The tools' logging.

use std::{
    ffi::OsStr,
    io::{IsTerminal, Write},
};

use tracing_subscriber::{EnvFilter, filter::ParseError};

/// The environment variable that turns on logging. Its value is an
/// `EnvFilter` directive.
const LOG: &str = "PROCPS_DARWIN_LOG";

/// When `PROCPS_DARWIN_LOG` is set, installs a subscriber that writes to
/// standard error, in a readable format on a terminal and as JSON otherwise.
/// A value that is not a valid `EnvFilter` directive is reported on standard
/// error, and no subscriber is installed. With the variable unset, the tools
/// print only what procps-ng prints.
///
/// ```
/// procps::observability::init();
/// ```
pub fn init() {
    let Some(directive) = std::env::var_os(LOG) else {
        return;
    };
    let filter = match filter(&directive) {
        Ok(filter) => filter,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "{LOG} is not a valid filter directive: {error}"
            );
            return;
        }
    };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);

    let installed = if std::io::stderr().is_terminal() {
        subscriber.pretty().try_init()
    } else {
        subscriber.json().try_init()
    };

    if let Err(error) = installed {
        let _ = writeln!(
            std::io::stderr(),
            "cannot install the logging subscriber: {error}"
        );
    }
}

/// The filter that `directive` describes.
fn filter(directive: &OsStr) -> Result<EnvFilter, ParseError> {
    EnvFilter::try_new(directive.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::filter;

    #[rstest]
    #[case::level("debug", Ok("debug"))]
    #[case::target("procps_core=trace", Ok("procps_core=trace"))]
    #[case::bad_level(
        "procps=loud",
        Err(r#"error parsing level filter: expected one of "off", "error", "warn", "info", "debug", "trace", or a number 0-5"#)
    )]
    fn a_directive_is_checked(#[case] directive: &str, #[case] expected: Result<&str, &str>) {
        let parsed = filter(directive.as_ref())
            .map(|filter| filter.to_string())
            .map_err(|error| error.to_string());

        assert_eq!(parsed, expected.map(str::to_owned).map_err(str::to_owned));
    }
}
