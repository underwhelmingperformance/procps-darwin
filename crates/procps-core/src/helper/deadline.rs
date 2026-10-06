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
/// A socket timeout limits each call separately, so a peer that sends one byte
/// at a time could keep a connection open for as long as it liked. This type
/// sets each call's timeout to the time that remains. On a connection that the
/// peer has closed, a read returns the data that the peer sent before it
/// closed and then end-of-file. A write fails with
/// [`io::ErrorKind::BrokenPipe`] or, when the peer closes during the write,
/// sometimes with [`io::ErrorKind::NotConnected`]. A write that fails with
/// `BrokenPipe` also raises `SIGPIPE`, unless the socket has `SO_NOSIGPIPE` or
/// the process ignores the signal.
///
/// ```
/// use std::{io::Read, os::unix::net::UnixStream, time::Duration};
///
/// use procps_core::helper::Deadline;
///
/// let (_ours, theirs) = UnixStream::pair()?;
/// let error = Deadline::new(&theirs, Duration::from_millis(10))
///     .read(&mut [0; 1])
///     .map_err(|error| error.kind());
///
/// assert_eq!(error, Err(std::io::ErrorKind::TimedOut));
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Deadline<'a> {
    stream: &'a UnixStream,
    until: Instant,
}

impl<'a> Deadline<'a> {
    /// Wraps `stream` so that every read and write must finish within `limit`
    /// from now.
    ///
    /// ```
    /// use std::{io::Write, os::unix::net::UnixStream, time::Duration};
    ///
    /// use procps_core::helper::Deadline;
    ///
    /// let (_ours, theirs) = UnixStream::pair()?;
    ///
    /// assert_eq!(
    ///     Deadline::new(&theirs, Duration::from_secs(1)).write(&[0])?,
    ///     1
    /// );
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[must_use]
    pub fn new(stream: &'a UnixStream, limit: Duration) -> Self {
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
            // The read cannot block then, and returns any data that the peer
            // sent before it closed.
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
            result => result?,
        }

        timed_out(self.stream.read(buffer))
    }
}

impl Write for Deadline<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.stream.set_write_timeout(Some(self.remaining()?)) {
            // Darwin refuses a timeout on a socket that the peer has closed.
            // The write cannot block then, and fails with `EPIPE`.
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
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
