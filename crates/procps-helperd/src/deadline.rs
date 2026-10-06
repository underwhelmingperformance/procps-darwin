// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

/// A connection whose reads and writes must all finish before a deadline.
///
/// A socket timeout limits each call separately, so a client that sends one
/// byte at a time could keep a connection open for as long as it liked. This
/// type sets each call's timeout to the time that remains.
pub(crate) struct Deadline<'a> {
    stream: &'a UnixStream,
    until: Instant,
}

impl<'a> Deadline<'a> {
    /// Wraps `stream` so that every read and write must finish within `limit`
    /// from now.
    pub(crate) fn new(stream: &'a UnixStream, limit: Duration) -> Self {
        let now = Instant::now();

        Self {
            stream,
            until: now.checked_add(limit).unwrap_or(now),
        }
    }

    /// The time that remains, or [`io::ErrorKind::TimedOut`] if the deadline
    /// has passed.
    fn remaining(&self) -> io::Result<Duration> {
        let remaining = self.until.saturating_duration_since(Instant::now());

        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }

        Ok(remaining)
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.stream.set_read_timeout(Some(self.remaining()?)) {
            // Darwin refuses a timeout on a socket that the peer has closed.
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => return Ok(0),
            result => result?,
        }

        timed_out(self.stream.read(buffer))
    }
}

impl Write for Deadline<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.stream.set_write_timeout(Some(self.remaining()?)) {
            // Darwin refuses a timeout on a socket that the peer has closed.
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            result => result?,
        }

        timed_out(self.stream.write(buffer))
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut stream = self.stream;

        stream.flush()
    }
}

/// `result`, with the error that a socket timeout produces replaced by
/// [`io::ErrorKind::TimedOut`]. Darwin reports a timeout as `EAGAIN`, which
/// Rust maps to [`io::ErrorKind::WouldBlock`].
fn timed_out<T>(result: io::Result<T>) -> io::Result<T> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Err(io::ErrorKind::TimedOut.into())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Read, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };

    use pretty_assertions::assert_eq;

    use super::Deadline;

    #[test]
    fn a_closed_connection_ends_reads_and_writes() -> io::Result<()> {
        let (client, theirs) = UnixStream::pair()?;
        drop(client);
        let mut connection = Deadline::new(&theirs, Duration::from_secs(1));

        let read = connection.read(&mut [0; 1]).map_err(|error| error.kind());
        let written = connection.write(&[0]).map_err(|error| error.kind());

        assert_eq!((read, written), (Ok(0), Err(io::ErrorKind::BrokenPipe)));

        Ok(())
    }

    #[test]
    fn a_connection_that_has_used_its_time_times_out() -> io::Result<()> {
        let (_client, theirs) = UnixStream::pair()?;
        let mut connection = Deadline::new(&theirs, Duration::ZERO);

        let read = connection.read(&mut [0; 1]).map_err(|error| error.kind());

        assert_eq!(read, Err(io::ErrorKind::TimedOut));

        Ok(())
    }
}
