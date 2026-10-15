// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standard output and standard error, buffered as C's standard I/O buffers
//! them, with procps-ng's handling of write errors.

use std::{
    io::{self, IsTerminal, Write},
    os::fd::AsFd,
};

use darwin_proc::{ClosedStreams, StandardStream};
use rustix::io::Errno;

/// The capacity of a buffer for a pipe or a file. glibc sizes a stream's
/// buffer by the block size of the file, which is 4096 bytes for a pipe, and
/// for a file on most Linux file systems. A terminal's block size is smaller.
const CAPACITY: usize = 4096;

/// The capacity of a buffer for a terminal, whose block size on Linux is 1024
/// bytes.
const TERMINAL_CAPACITY: usize = 1024;

/// When a stream writes the data that it has buffered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Buffering {
    /// At once, as C does for standard error.
    Unbuffered,
    /// At the end of each line, as C does for a terminal.
    Line,
    /// When the buffer is full, as C does for a pipe or a file.
    Full,
}

/// A buffered writer for a file descriptor. The writer remembers the error
/// from the last failed write, which C's `errno` contains when procps-ng's
/// `close_stream` reads it.
///
/// A write fails with `EBADF` when a standard stream is open only for reading,
/// and Rust's standard streams treat that failure as a success. procps-ng
/// reports the failure, so this writer calls `write` itself.
#[derive(Debug)]
struct Stream<F> {
    fd: F,
    buffering: Buffering,
    buffer: Vec<u8>,
    error: Option<Errno>,
    /// The error that every non-empty write fails with, without reaching the
    /// file.
    refusal: Option<Errno>,
    /// The error from closing the stream, which C's `fclose` reports after it
    /// has written the buffer.
    close_error: Option<Errno>,
}

impl<F: AsFd> Stream<F> {
    const fn new(fd: F, buffering: Buffering) -> Self {
        Self {
            fd,
            buffering,
            buffer: Vec::new(),
            error: None,
            refusal: None,
            close_error: None,
        }
    }

    /// The stream, with every non-empty write failing with `refusal` if it is
    /// set.
    fn refusing(self, refusal: Option<Errno>) -> Self {
        Self { refusal, ..self }
    }

    /// The stream, with closing it failing with `close_error` if it is set.
    fn closing_with(self, close_error: Option<Errno>) -> Self {
        Self {
            close_error,
            ..self
        }
    }

    /// Writes all of `bytes`, and records the error if a write fails.
    fn write_through(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        if let Some(errno) = self.refusal.filter(|_| !bytes.is_empty()) {
            self.error = Some(errno);
            return Err(errno.into());
        }

        while !bytes.is_empty() {
            match rustix::io::write(&self.fd, bytes) {
                Ok(0) => {
                    self.error = Some(Errno::IO);
                    return Err(io::ErrorKind::WriteZero.into());
                }
                Ok(written) => bytes = &bytes[written..],
                Err(Errno::INTR) => {}
                Err(errno) => {
                    self.error = Some(errno);
                    return Err(errno.into());
                }
            }
        }

        Ok(())
    }

    /// Writes the first `end` bytes of the buffer. glibc's standard I/O
    /// discards the data of a failed write, so the bytes leave the buffer
    /// either way.
    fn write_buffered(&mut self, end: usize) -> io::Result<()> {
        let pending: Vec<u8> = self.buffer.drain(..end).collect();

        self.write_through(&pending)
    }

    /// Writes `bytes` to a buffered stream, as glibc's `_IO_new_file_xsputn`
    /// does. The bytes that fit go in the buffer. On a line-buffered stream,
    /// when all the bytes fit and contain a newline, only the bytes up to the
    /// last newline go in. The buffer is written only if bytes remain, or in
    /// that line-buffered case. Then every whole buffer's worth of the rest is
    /// written directly, and the remainder goes in the buffer. If a write
    /// fails, the rest of `bytes` is discarded.
    fn write_buffered_call(&mut self, bytes: &[u8], capacity: usize) -> io::Result<()> {
        let space = capacity.saturating_sub(self.buffer.len());
        let newline = match self.buffering {
            Buffering::Line if bytes.len() <= space => {
                bytes.iter().rposition(|&byte| byte == b'\n')
            }
            _ => None,
        };
        let fit = newline.map_or(space.min(bytes.len()), |newline| newline + 1);
        let (buffered, rest) = bytes.split_at(fit);
        self.buffer.extend_from_slice(buffered);

        if rest.is_empty() && newline.is_none() {
            return Ok(());
        }

        self.write_buffered(self.buffer.len())?;
        let (direct, remainder) = rest.split_at(rest.len() - rest.len() % capacity);
        self.write_through(direct)?;
        self.buffer.extend_from_slice(remainder);

        Ok(())
    }
}

impl<F: AsFd> Write for Stream<F> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.buffering {
            Buffering::Unbuffered => self.write_through(bytes)?,
            Buffering::Line => self.write_buffered_call(bytes, TERMINAL_CAPACITY)?,
            Buffering::Full => self.write_buffered_call(bytes, CAPACITY)?,
        }

        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.write_buffered(self.buffer.len())
    }
}

/// A failed write that procps-ng's `close_stdout` reports.
#[derive(Debug, thiserror::Error)]
pub enum WriteFailure {
    /// A write to standard output failed. The error comes from the final
    /// flush or from closing the stream, and is `None` if only an earlier
    /// write failed. procps-ng prints the description of the error only when
    /// `fclose` fails, because C's `errno` then comes from `fclose`.
    #[error("write error")]
    Stdout(#[source] Option<io::Error>),
    /// A write to standard error failed. procps-ng exits with status 1 and
    /// prints nothing.
    #[error("cannot write to standard error")]
    Stderr,
}

/// The tool's standard output and standard error.
///
/// ```
/// use std::io::Write;
///
/// let mut output = procps::output::Output::new();
/// let (stdout, _) = output.streams();
/// write!(stdout, "")?;
///
/// assert!(output.close().is_ok());
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Output {
    stdout: Stream<io::StdoutLock<'static>>,
    stderr: Stream<io::StderrLock<'static>>,
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

impl Output {
    /// The process's standard output, which is line-buffered on a terminal
    /// and fully buffered otherwise, and its standard error, which is
    /// unbuffered.
    ///
    /// ```
    /// let _output = procps::output::Output::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        let stdout = io::stdout();
        let buffering = if stdout.is_terminal() {
            Buffering::Line
        } else {
            Buffering::Full
        };

        // procps-ng's `main` calls `epoll_create` before anything else, so the
        // epoll descriptor takes the lowest standard descriptor that was
        // closed at start-up, and writes to that stream fail with `EINVAL`.
        // When standard input and standard output were both closed, the epoll
        // descriptor takes standard input's, and standard output's stays
        // closed, so `fclose` fails with `EBADF`.
        let closed = ClosedStreams::at_start();
        let refusal = |stream| (closed.first() == Some(stream)).then_some(Errno::INVAL);
        let close_error = (closed.first() == Some(StandardStream::Input)
            && closed.contains(StandardStream::Output))
        .then_some(Errno::BADF);

        Self {
            stdout: Stream::new(stdout.lock(), buffering)
                .refusing(refusal(StandardStream::Output))
                .closing_with(close_error),
            stderr: Stream::new(io::stderr().lock(), Buffering::Unbuffered)
                .refusing(refusal(StandardStream::Error)),
        }
    }

    /// Standard output and standard error.
    ///
    /// ```
    /// use std::io::Write;
    ///
    /// let mut output = procps::output::Output::new();
    /// let (_, stderr) = output.streams();
    ///
    /// stderr.write_all(b"")?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn streams(&mut self) -> (&mut impl Write, &mut impl Write) {
        (&mut self.stdout, &mut self.stderr)
    }

    /// Flushes standard output and reports the failed writes that procps-ng's
    /// `close_stdout` reports. A failure is ignored when the last failed write
    /// to standard output, or the final flush, failed with `EPIPE`.
    ///
    /// ```
    /// let output = procps::output::Output::new();
    ///
    /// assert!(output.close().is_ok());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`WriteFailure::Stdout`] if a write to standard output failed,
    /// unless the last failure was `EPIPE`, and otherwise
    /// [`WriteFailure::Stderr`] if a write to standard error failed.
    pub fn close(self) -> Result<(), WriteFailure> {
        close(self.stdout, &self.stderr)
    }
}

/// Flushes `stdout` and reports the failed writes to `stdout` and `stderr`
/// that procps-ng's `close_stdout` reports.
fn close<O: AsFd, E: AsFd>(mut stdout: Stream<O>, stderr: &Stream<E>) -> Result<(), WriteFailure> {
    let flushed = stdout.flush();

    if let Some(last) = stdout.error {
        // `close_stream` leaves `errno` from the failed `fclose`, or from
        // the last failed write only if that write failed with `EPIPE`.
        let error = match (flushed, stdout.close_error) {
            (Err(error), _) => Some(error),
            (Ok(()), Some(close_error)) => Some(close_error.into()),
            (Ok(()), None) if last == Errno::PIPE => Some(last.into()),
            (Ok(()), None) => None,
        };
        let closed_pipe = error
            .as_ref()
            .and_then(io::Error::raw_os_error)
            .is_some_and(|code| code == Errno::PIPE.raw_os_error());

        if !closed_pipe {
            return Err(WriteFailure::Stdout(error));
        }
    }

    if stderr.error.is_some() {
        return Err(WriteFailure::Stderr);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{PipeWriter, Write},
    };

    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use rustix::io::Errno;

    use super::{Buffering, CAPACITY, Stream, WriteFailure, close};

    /// The length of the file after each of `writes`.
    fn written(
        buffering: Buffering,
        writes: &[&[u8]],
    ) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("output");
        let mut stream = Stream::new(File::create(&path)?, buffering);
        let mut lengths = Vec::new();

        for bytes in writes {
            stream.write_all(bytes)?;
            lengths.push(std::fs::metadata(&path)?.len());
        }

        Ok(lengths)
    }

    #[rstest]
    #[case::unbuffered(Buffering::Unbuffered, vec![1, 3])]
    #[case::line(Buffering::Line, vec![0, 3])]
    #[case::full(Buffering::Full, vec![0, 0])]
    fn a_stream_writes_as_c_buffers(
        #[case] buffering: Buffering,
        #[case] expected: Vec<u64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(written(buffering, &[b"a", b"b\n"])?, expected);

        Ok(())
    }

    #[test]
    fn a_full_buffer_is_written_and_the_rest_is_buffered() -> Result<(), Box<dyn std::error::Error>>
    {
        let block = vec![b'x'; CAPACITY];

        assert_eq!(
            written(Buffering::Full, &[b"a", &block])?,
            vec![0, u64::try_from(CAPACITY)?]
        );

        Ok(())
    }

    /// Where a test's standard output goes.
    #[derive(Clone, Copy, Debug)]
    enum Target {
        /// A file that accepts writes.
        File,
        /// A file open only for reading, so writes fail with `EBADF`.
        ReadOnly,
        /// A pipe whose read end is closed, so writes fail with `EPIPE`.
        ClosedPipe,
    }

    /// The writer for `target`, as a file descriptor.
    fn open(target: Target) -> Result<std::os::fd::OwnedFd, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("output");
        let file = File::create(&path)?;

        Ok(match target {
            Target::File => file.into(),
            Target::ReadOnly => File::open(&path)?.into(),
            Target::ClosedPipe => {
                let (reader, writer): (_, PipeWriter) = std::io::pipe()?;
                drop(reader);
                writer.into()
            }
        })
    }

    /// What `close` reports: the error number of a failed write to standard
    /// output, if any, or that a write to standard error failed.
    #[derive(Debug, PartialEq, Eq)]
    enum Reported {
        Stdout(Option<i32>),
        Stderr,
    }

    fn reported(result: Result<(), WriteFailure>) -> Result<(), Reported> {
        result.map_err(|failure| match failure {
            WriteFailure::Stdout(error) => {
                Reported::Stdout(error.and_then(|error| error.raw_os_error()))
            }
            WriteFailure::Stderr => Reported::Stderr,
        })
    }

    #[rstest]
    #[case::earlier_write(
        Target::ReadOnly,
        Buffering::Unbuffered,
        Target::File,
        Err(Reported::Stdout(None))
    )]
    #[case::final_flush(
        Target::ReadOnly,
        Buffering::Full,
        Target::File,
        Err(Reported::Stdout(Some(libc::EBADF)))
    )]
    #[case::earlier_closed_pipe(Target::ClosedPipe, Buffering::Unbuffered, Target::File, Ok(()))]
    #[case::closed_pipe_at_flush(Target::ClosedPipe, Buffering::Full, Target::File, Ok(()))]
    #[case::stderr(Target::File, Buffering::Full, Target::ReadOnly, Err(Reported::Stderr))]
    fn close_reports_what_close_stdout_reports(
        #[case] stdout: Target,
        #[case] buffering: Buffering,
        #[case] stderr: Target,
        #[case] expected: Result<(), Reported>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut stdout = Stream::new(open(stdout)?, buffering);
        let mut stderr = Stream::new(open(stderr)?, Buffering::Unbuffered);
        let _ = stdout.write_all(b"x\n");
        let _ = stderr.write_all(b"y\n");

        assert_eq!(reported(close(stdout, &stderr)), expected);

        Ok(())
    }

    // A refused stream behaves as procps-ng's epoll descriptor does, and a
    // stream with a close error as a descriptor that stays closed.
    #[rstest]
    #[case::refused_without_output(Some(Errno::INVAL), None, b"", Ok(()))]
    #[case::refused(
        Some(Errno::INVAL),
        None,
        b"x\n",
        Err(Reported::Stdout(Some(libc::EINVAL)))
    )]
    #[case::closed_without_output(None, Some(Errno::BADF), b"", Ok(()))]
    #[case::closed_after_an_earlier_write(
        None,
        Some(Errno::BADF),
        &[b'x'; CAPACITY + 1],
        Err(Reported::Stdout(Some(libc::EBADF)))
    )]
    fn close_reports_a_stream_closed_at_start_up(
        #[case] refusal: Option<Errno>,
        #[case] close_error: Option<Errno>,
        #[case] bytes: &[u8],
        #[case] expected: Result<(), Reported>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let target = if refusal.is_some() {
            Target::File
        } else {
            Target::ReadOnly
        };
        let mut stdout = Stream::new(open(target)?, Buffering::Full)
            .refusing(refusal)
            .closing_with(close_error);
        let stderr = Stream::new(open(Target::File)?, Buffering::Unbuffered);
        let _ = stdout.write_all(bytes);

        assert_eq!(reported(close(stdout, &stderr)), expected);

        Ok(())
    }

    #[rstest]
    #[case::closed_pipe_last(Target::ReadOnly, Target::ClosedPipe, Ok(()))]
    #[case::closed_pipe_first(Target::ClosedPipe, Target::ReadOnly, Err(Reported::Stdout(None)))]
    fn close_reports_the_last_failed_write(
        #[case] first: Target,
        #[case] last: Target,
        #[case] expected: Result<(), Reported>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut stdout = Stream::new(open(first)?, Buffering::Unbuffered);
        let stderr = Stream::new(open(Target::File)?, Buffering::Unbuffered);
        let _ = stdout.write_all(b"x\n");
        rustix::io::dup2(open(last)?, &mut stdout.fd)?;
        let _ = stdout.write_all(b"y\n");

        assert_eq!(reported(close(stdout, &stderr)), expected);

        Ok(())
    }

    // On a terminal, glibc's buffer has 1024 bytes. A call that fills it
    // exactly stays buffered, and a newline in a call that fits writes the
    // buffer.
    #[rstest]
    #[case::fills_the_buffer(vec![vec![b'x'; 1024]], vec![0])]
    #[case::then_one_more(vec![vec![b'x'; 1024], b"y".to_vec()], vec![0, 1024])]
    #[case::newline(vec![b"ab".to_vec(), b"c\nd".to_vec()], vec![0, 4])]
    #[case::newline_that_fills_the_buffer(vec![vec![b'x'; 1022], b"x\n".to_vec()], vec![0, 1024])]
    fn a_terminal_s_buffer_is_written_as_glibc_writes_it(
        #[case] calls: Vec<Vec<u8>>,
        #[case] expected: Vec<u64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let calls: Vec<&[u8]> = calls.iter().map(Vec::as_slice).collect();

        assert_eq!(written(Buffering::Line, &calls)?, expected);

        Ok(())
    }

    // Each write is one `printf` call of the given length, to standard output
    // that is open only for reading. glibc flushes a full buffer only when a
    // call has more bytes for it, and discards the rest of a call whose flush
    // fails. procps-ng adds the error's description only when bytes are still
    // buffered at exit.
    #[rstest]
    #[case::fits(&[4095], Reported::Stdout(Some(libc::EBADF)))]
    #[case::fills_the_buffer(&[4096], Reported::Stdout(Some(libc::EBADF)))]
    #[case::overflows(&[4097], Reported::Stdout(None))]
    #[case::fills_then_overflows(&[4096, 1], Reported::Stdout(None))]
    #[case::later_call(&[8190, 2], Reported::Stdout(Some(libc::EBADF)))]
    #[case::several_buffers(&[12_288, 2], Reported::Stdout(Some(libc::EBADF)))]
    fn the_buffer_is_written_as_glibc_writes_it(
        #[case] calls: &[usize],
        #[case] expected: Reported,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut stdout = Stream::new(open(Target::ReadOnly)?, Buffering::Full);
        let stderr = Stream::new(open(Target::File)?, Buffering::Unbuffered);

        for &length in calls {
            let _ = stdout.write_all(&vec![b'x'; length]);
        }

        assert_eq!(reported(close(stdout, &stderr)), Err(expected));

        Ok(())
    }
}
