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
/// use procps_core::helper::ProtocolError;
/// use procps_helperd::{ErrorChain, ServeError};
///
/// let error = ServeError::Protocol(ProtocolError::Io(io::ErrorKind::TimedOut.into()));
///
/// assert_eq!(
///     ErrorChain(&error).to_string(),
///     "cannot exchange messages with the client: \
///      cannot read from or write to the helper connection: timed out"
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

#[cfg(test)]
mod tests {
    use std::io;

    use pretty_assertions::assert_eq;

    use super::ErrorChain;
    use crate::DaemonError;

    #[test]
    fn a_source_in_its_parents_message_appears_once() {
        let error = DaemonError::Launchd(darwin_proc::Error::Os {
            call: darwin_proc::Call::LaunchdSockets,
            source: io::Error::from_raw_os_error(3),
        });

        assert_eq!(
            ErrorChain(&error).to_string(),
            "cannot take the socket from launchd: launch_activate_socket: No such process (os \
             error 3)"
        );
    }
}
