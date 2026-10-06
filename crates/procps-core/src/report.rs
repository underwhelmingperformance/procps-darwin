// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{error::Error, fmt};

/// An error and its sources, separated by colons, for the logs. The JSON
/// formatter records only an error's own message, which leaves out the
/// underlying cause, such as the errno of a failed call. A source whose
/// message already ends the previous message is left out, because some errors
/// include their source in their own message.
///
/// ```
/// use std::io;
///
/// use procps_core::{ErrorChain, SourceError};
///
/// let error = SourceError::ProcessTable(darwin_proc::Error::Os {
///     call: darwin_proc::Call::ProcessTable,
///     source: io::Error::from_raw_os_error(1),
/// });
///
/// assert_eq!(
///     ErrorChain(&error).to_string(),
///     "cannot list processes: sysctl kern.proc: Operation not permitted (os error 1)"
/// );
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ErrorChain<'a>(pub &'a dyn Error);

impl fmt::Display for ErrorChain<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut previous = self.0.to_string();
        formatter.write_str(&previous)?;

        let mut source = self.0.source();

        while let Some(error) = source {
            let message = error.to_string();

            if !previous.ends_with(&message) {
                write!(formatter, ": {message}")?;
            }

            previous = message;
            source = error.source();
        }

        Ok(())
    }
}
