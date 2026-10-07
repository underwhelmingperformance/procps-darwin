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

use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    os::fd::{AsFd, BorrowedFd},
    path::Path,
};

use darwin_proc::ClosedDescriptors;
use rustix::{
    fs::{FileType, FlockOperation, fcntl_lock, flock, fstat},
    io::Errno,
};

use crate::{c_number::strtol, pgrep::PgrepError};

/// The longest line that procps-ng reads from a pid file: its buffer has 256
/// bytes, and `fgets` keeps one for the terminating NUL.
const LINE_LENGTH: u64 = 255;

/// Reads the pid at the start of the pid file `path`, where `-` is standard
/// input, as procps-ng's `read_pidfile` does. With `lock`, another process
/// must have locked the file.
pub(crate) fn read(path: &Path, lock: bool) -> Result<i64, PgrepError> {
    if path.as_os_str() == "-" {
        let stdin = io::stdin();

        return read_from(stdin.as_fd(), stdin.lock(), lock);
    }

    if let Some(error) = closed_descriptor_error(path, ClosedDescriptors::at_start()) {
        return Err(PgrepError::OpenPidfile(error));
    }

    let file = File::open(path).map_err(|error| {
        // Opening a socket fails with `EOPNOTSUPP` on Darwin and with `ENXIO`
        // on Linux. procps-ng prints `ENXIO`'s description.
        if error.raw_os_error() == Some(libc::EOPNOTSUPP) {
            return PgrepError::OpenPidfile(io::Error::from_raw_os_error(libc::ENXIO));
        }

        PgrepError::OpenPidfile(error)
    })?;

    read_from(file.as_fd(), BufReader::new(&file), lock)
}

/// The error that opening `path` gives on Linux if `path` refers to a
/// descriptor in `closed`, such as `/dev/stdin` after `<&-`. procps-ng's epoll
/// descriptor takes the lowest descriptor that was closed when it started, and
/// Linux refuses to open an epoll descriptor by its path with `ENXIO`. The
/// other closed descriptors stay closed, so their paths do not exist. On
/// Darwin, the closed standard descriptors are open on `/dev/null`, and opening
/// the path of any other closed descriptor fails with `EBADF`.
///
/// Linux's procfs accepts only the canonical decimal form of a descriptor's
/// number, so on Linux a name in `/dev/fd` such as `03` or `+3` does not exist,
/// although Darwin opens `/dev/fd/03` as descriptor 3.
fn closed_descriptor_error(path: &Path, closed: ClosedDescriptors) -> Option<io::Error> {
    let DescriptorName::Number(descriptor) = descriptor(path)? else {
        return Some(io::Error::from_raw_os_error(libc::ENOENT));
    };

    if !closed.contains(descriptor) {
        return None;
    }

    let errno = if closed.lowest() == Some(descriptor) {
        libc::ENXIO
    } else {
        libc::ENOENT
    };

    Some(io::Error::from_raw_os_error(errno))
}

/// The most symbolic links that [`descriptor`] follows, as many as Darwin's
/// `MAXSYMLINKS`.
const MAX_SYMBOLIC_LINKS: usize = 32;

/// A name in `/dev/fd`.
enum DescriptorName {
    /// The canonical decimal form of a descriptor's number.
    Number(libc::c_int),
    /// Any other name, such as `03`, `+3` or `x`.
    Other,
}

/// The name in `/dev/fd` that `path` refers to, such as 0 for `/dev/stdin`, or
/// `None` if `path` is not in `/dev/fd`. This follows symbolic links itself,
/// because Darwin's `realpath` resolves `/dev/fd/1` to a path that ends with
/// the name of the file open on descriptor 1.
fn descriptor(path: &Path) -> Option<DescriptorName> {
    let mut path = path.to_owned();

    for _ in 0..MAX_SYMBOLIC_LINKS {
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let parent = std::fs::canonicalize(parent).ok()?;
        let name = path.file_name()?;

        let Ok(target) = std::fs::read_link(parent.join(name)) else {
            if parent != Path::new("/dev/fd") {
                return None;
            }

            let number = name.to_str().and_then(|text| {
                text.parse::<libc::c_int>()
                    .ok()
                    .filter(|n| n.to_string() == text)
            });

            return Some(number.map_or(DescriptorName::Other, DescriptorName::Number));
        };

        path = parent.join(target);
    }

    None
}

fn read_from(fd: BorrowedFd<'_>, reader: impl BufRead, lock: bool) -> Result<i64, PgrepError> {
    if lock {
        check_lock(fd).map_err(|errno| PgrepError::UnlockedPidfile(errno.map(Into::into)))?;
    }

    let mut line = Vec::new();

    if reader
        .take(LINE_LENGTH)
        .read_until(b'\n', &mut line)
        .is_err()
    {
        return Err(PgrepError::InvalidPidfile);
    }

    // `fgets` gives a C string, which ends at the first NUL.
    if let Some(nul) = line.iter().position(|&byte| byte == 0) {
        line.truncate(nul);
    }

    let Some(number) = strtol(&line) else {
        return Err(PgrepError::InvalidPidfile);
    };
    let ends_well = line
        .get(number.length)
        .is_none_or(|&byte| matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'));

    if number.overflowed || !ends_well {
        return Err(PgrepError::InvalidPidfile);
    }

    Ok(number.value)
}

/// Checks that another process has locked the file that `fd` refers to, as
/// procps-ng's `has_flock` and `has_fcntl` do. Each makes a lock call that
/// asks for a shared lock without blocking. The file is locked if a lock call
/// fails because another process has an exclusive lock. Otherwise this returns
/// the error that procps-ng reports: the error of the last lock call that
/// failed, or `None` if both lock calls succeeded.
///
/// Linux locks a pipe as it locks a file, so on Linux both lock calls succeed
/// for a pid file on standard input from a pipe. Darwin rejects both calls on a
/// pipe or a socket, with `ENOTSUP` or `EOPNOTSUPP` from `flock` and `EBADF`
/// from `fcntl`. For those file types, this treats those errors as granted
/// locks, as on Linux.
fn check_lock(fd: BorrowedFd<'_>) -> Result<(), Option<Errno>> {
    let unlockable_type = fstat(fd).is_ok_and(|stat| {
        matches!(
            FileType::from_raw_mode(stat.st_mode),
            FileType::Fifo | FileType::Socket
        )
    });
    let granted = |result: Result<(), Errno>| match result {
        Err(Errno::NOTSUP | Errno::OPNOTSUPP | Errno::BADF) if unlockable_type => Ok(()),
        result => result,
    };
    let flocked = granted(flock(fd, FlockOperation::NonBlockingLockShared));

    if flocked == Err(Errno::WOULDBLOCK) {
        return Ok(());
    }

    match granted(fcntl_lock(fd, FlockOperation::NonBlockingLockShared)) {
        Err(Errno::ACCESS | Errno::AGAIN) => Ok(()),
        Err(errno) => Err(Some(errno)),
        Ok(()) => Err(flocked.err()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{BufReader, Write},
        os::fd::AsFd,
    };

    use assert_matches::assert_matches;
    use darwin_proc::ClosedDescriptors;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use rustix::fs::{FlockOperation, flock};

    use super::{closed_descriptor_error, read, read_from};
    use crate::pgrep::PgrepError;

    #[rstest]
    #[case::newline("123\n", Ok(123))]
    #[case::end_of_file("123", Ok(123))]
    #[case::space_after(" 42 daemon\n", Ok(42))]
    #[case::tab_after("42\t", Ok(42))]
    #[case::vertical_tab_after("42\x0b", Ok(42))]
    #[case::form_feed_after("42\x0c", Ok(42))]
    #[case::carriage_return_after("42\r", Ok(42))]
    #[case::form_feed_before("\x0c42\n", Ok(42))]
    #[case::second_line("7\n8\n", Ok(7))]
    #[case::letters_after("12x\n", Err("pidfile not valid"))]
    #[case::not_a_number("x\n", Err("pidfile not valid"))]
    #[case::empty("", Err("pidfile not valid"))]
    #[case::too_large("99999999999999999999\n", Err("pidfile not valid"))]
    #[case::cut_at_a_nul("4\x005\n", Ok(4))]
    #[case::pid_within_the_line(format!("{}7\n", " ".repeat(254)), Ok(7))]
    #[case::pid_beyond_the_line(format!("{}7\n", " ".repeat(255)), Err("pidfile not valid"))]
    fn a_pid_file_starts_with_a_pid(
        #[case] contents: String,
        #[case] expected: Result<i64, &str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon.pid");
        std::fs::write(&path, contents)?;

        assert_eq!(
            read(&path, false).map_err(|error| error.to_string()),
            expected.map_err(str::to_owned)
        );

        Ok(())
    }

    #[rstest]
    #[case::missing("/nonexistent/daemon.pid")]
    #[case::dash_directory("-/")]
    fn a_missing_pid_file_cannot_be_opened(#[case] path: &str) {
        assert_matches!(
            read(path.as_ref(), false),
            Err(PgrepError::OpenPidfile(error)) if error.raw_os_error() == Some(libc::ENOENT)
        );
    }

    #[rstest]
    #[case::input_first("/dev/stdin", &[0], Some(libc::ENXIO))]
    #[case::output_first("/dev/fd/1", &[1], Some(libc::ENXIO))]
    #[case::output_after_input("/dev/fd/1", &[0, 1], Some(libc::ENOENT))]
    #[case::open("/dev/stdin", &[1], None)]
    #[case::other_file("/dev/null", &[0], None)]
    #[case::first_after_the_standard_descriptors("/dev/fd/3", &[3, 4], Some(libc::ENXIO))]
    #[case::later_after_the_standard_descriptors("/dev/fd/4", &[3, 4], Some(libc::ENOENT))]
    #[case::leading_zero("/dev/fd/03", &[3, 4], Some(libc::ENOENT))]
    #[case::sign("/dev/fd/+3", &[], Some(libc::ENOENT))]
    #[case::leading_zero_of_an_open_descriptor("/dev/fd/00", &[], Some(libc::ENOENT))]
    fn a_closed_descriptor_cannot_be_opened_as_on_linux(
        #[case] path: &str,
        #[case] closed: &[libc::c_int],
        #[case] expected: Option<i32>,
    ) {
        let closed: ClosedDescriptors = closed.iter().copied().collect();

        assert_eq!(
            closed_descriptor_error(path.as_ref(), closed).and_then(|error| error.raw_os_error()),
            expected
        );
    }

    #[test]
    fn a_socket_cannot_be_opened_as_on_linux() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path)?;

        assert_matches!(
            read(&path, false),
            Err(PgrepError::OpenPidfile(error)) if error.raw_os_error() == Some(libc::ENXIO)
        );

        Ok(())
    }

    #[test]
    fn a_lock_is_required_only_with_lock() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon.pid");
        File::create(&path)?.write_all(b"5\n")?;

        let unlocked_without_lock = read(&path, false).map_err(|error| error.to_string());
        let unlocked = read(&path, true).map_err(|error| error.to_string());

        let holder = File::open(&path)?;
        flock(&holder, FlockOperation::NonBlockingLockExclusive)?;
        let locked = read(&path, true).map_err(|error| error.to_string());

        assert_eq!(
            (unlocked_without_lock, unlocked, locked),
            (
                Ok(5),
                Err("Locking check for pidfile failed: Success".to_owned()),
                Ok(5)
            )
        );

        Ok(())
    }

    #[test]
    fn a_pipe_is_unlocked_as_on_linux() -> Result<(), Box<dyn std::error::Error>> {
        let (reader, mut writer) = std::io::pipe()?;
        writer.write_all(b"5\n")?;
        drop(writer);

        assert_eq!(
            read_from(reader.as_fd(), BufReader::new(&reader), true)
                .map_err(|error| error.to_string()),
            Err("Locking check for pidfile failed: Success".to_owned())
        );

        Ok(())
    }

    #[test]
    fn a_write_only_descriptor_is_a_failed_lock_check() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon.pid");
        std::fs::write(&path, "5\n")?;
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;

        assert_eq!(
            read_from(file.as_fd(), BufReader::new(&file), true).map_err(|error| error.to_string()),
            Err("Locking check for pidfile failed: Bad file descriptor".to_owned())
        );

        Ok(())
    }
}
