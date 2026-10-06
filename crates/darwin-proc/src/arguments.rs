// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{ffi::OsString, io, os::unix::ffi::OsStringExt, path::PathBuf};

use crate::{
    Call, Error, Pid, Status,
    sysctl::{sysctl, sysctl_value},
};

/// The executable path, arguments and environment of a process, from the
/// `kern.procargs2` sysctl.
///
/// macOS 27 withholds the environment of Apple's platform binaries, and
/// possibly of other processes, from every process except themselves.
/// `environment` is then empty, although the read succeeded. The exception is
/// a process whose `argv[0]` is empty: the kernel then also returns its first
/// environment string. macOS 26 returns the environment of platform binaries.
///
/// ```
/// use darwin_proc::Pid;
///
/// let arguments = Pid::current().arguments()?;
///
/// assert_eq!(arguments.arguments, std::env::args_os().collect::<Vec<_>>());
/// # Ok::<(), darwin_proc::Error>(())
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Arguments {
    /// The path that the process executed.
    pub executable: PathBuf,
    /// The arguments, starting with `argv[0]`.
    pub arguments: Vec<OsString>,
    /// The environment, as `NAME=value` strings.
    pub environment: Vec<OsString>,
}

/// A `kern.procargs2` buffer that does not start with a valid argument count.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("the kern.procargs2 buffer does not start with a valid argument count")]
pub struct MalformedArguments;

impl TryFrom<&[u8]> for Arguments {
    type Error = MalformedArguments;

    /// Parses a `kern.procargs2` buffer: the argument count as a native
    /// `int`, the executable path padded with NULs to a multiple of 8 bytes,
    /// then each argument and each environment string with a NUL after it.
    /// The `apple[]` strings follow the environment and are not part of it,
    /// so the environment ends before them: at the empty string that padding
    /// or libSystem leaves, or at the first `apple[]` string, which starts
    /// with `pfz=0x` on an 8-byte boundary.
    ///
    /// A process can overwrite its argument strings, so the buffer may
    /// contain fewer strings than the count says. A string that the buffer cuts
    /// off is kept up to the end of the buffer.
    ///
    /// ```
    /// use darwin_proc::Arguments;
    ///
    /// let mut buffer = 1_i32.to_ne_bytes().to_vec();
    /// buffer.extend(b"/bin/sh\0sh\0");
    ///
    /// let arguments = Arguments::try_from(buffer.as_slice())?;
    ///
    /// assert_eq!(arguments.arguments, ["sh"]);
    /// # Ok::<(), darwin_proc::MalformedArguments>(())
    /// ```
    fn try_from(buffer: &[u8]) -> Result<Self, Self::Error> {
        let (count, rest) = buffer
            .split_first_chunk::<{ size_of::<libc::c_int>() }>()
            .ok_or(MalformedArguments)?;
        let count =
            usize::try_from(libc::c_int::from_ne_bytes(*count)).map_err(|_| MalformedArguments)?;
        let mut strings = Strings { rest, offset: 0 };

        let executable = strings.next().unwrap_or_default();
        strings.skip_padding();
        let arguments: Vec<OsString> = strings.by_ref().take(count).collect();
        let mut environment = Vec::new();

        while !strings.at_apple_strings() {
            match strings.next() {
                Some(entry) if !entry.is_empty() => environment.push(entry),
                _ => break,
            }
        }

        Ok(Self {
            executable: PathBuf::from(executable),
            arguments,
            environment,
        })
    }
}

/// The start of the first `apple[]` string, which XNU's
/// `exec_add_apple_strings` always writes: the key `pfz=` and the `0x` prefix
/// of a hexadecimal address.
const FIRST_APPLE_STRING: &[u8] = b"pfz=0x";

/// The NUL-terminated strings of a `kern.procargs2` buffer, after its argument
/// count.
struct Strings<'a> {
    /// The bytes that have not been read.
    rest: &'a [u8],
    /// The offset of `rest` from the start of the strings.
    offset: usize,
}

impl Strings<'_> {
    /// Skips the NULs that pad the strings read so far to a multiple of 8
    /// bytes.
    ///
    /// Skipping every NUL would also skip an empty argument after the
    /// padding.
    fn skip_padding(&mut self) {
        let padding = self.offset.next_multiple_of(8) - self.offset;
        let skipped = self
            .rest
            .iter()
            .take(padding)
            .position(|&byte| byte != 0)
            .unwrap_or(padding.min(self.rest.len()));

        self.advance(skipped);
    }

    /// Whether the next string is the first `apple[]` string.
    ///
    /// XNU pads the end of the argument and environment strings with NULs to
    /// a multiple of 8 bytes, then writes the `apple[]` strings. The padding
    /// leaves an empty string after the environment unless the strings already
    /// end on a boundary. libSystem also zeroes the first `apple[]` strings
    /// early in a process's start-up, which leaves more empty strings. When the
    /// strings end on a boundary and libSystem has not yet zeroed anything, no
    /// empty string separates the environment from the `apple[]` strings, so
    /// the parser must recognise the first `apple[]` string.
    ///
    /// An environment variable named `pfz` whose value starts with `0x`
    /// matches only if it also starts on a boundary. The environment would
    /// then end before it.
    fn at_apple_strings(&self) -> bool {
        self.offset.is_multiple_of(8) && self.rest.starts_with(FIRST_APPLE_STRING)
    }

    fn advance(&mut self, count: usize) {
        self.rest = self.rest.get(count..).unwrap_or_default();
        self.offset += count;
    }
}

impl Iterator for Strings<'_> {
    type Item = OsString;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }

        let end = self
            .rest
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(self.rest.len());
        let string = OsString::from_vec(self.rest[..end].to_vec());
        self.advance(end + 1);

        Some(string)
    }
}

impl Pid {
    /// Reads the process's executable path, arguments and, unless macOS
    /// withholds it, environment.
    ///
    /// ```
    /// use darwin_proc::Pid;
    ///
    /// let arguments = Pid::current().arguments()?;
    ///
    /// assert_eq!(arguments.executable, std::env::current_exe()?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Exited`] if the process does not exist,
    /// [`Error::Unsupported`] for `kernel_task` and zombies, which have no
    /// arguments, [`Error::Denied`] for another user's process, or another
    /// error if the sysctl fails.
    #[tracing::instrument(level = "debug", err(level = "debug"))]
    pub fn arguments(self) -> Result<Arguments, Error> {
        let os_error = |source| Error::Os {
            call: Call::Arguments,
            source,
        };
        let size: libc::c_int = sysctl_value(c"kern.argmax").map_err(os_error)?;
        let mut buffer = vec![
            0_u8;
            usize::try_from(size)
                .map_err(io::Error::other)
                .map_err(os_error)?
        ];
        let mut size = buffer.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, self.as_raw()];

        if let Err(source) = sysctl(&mut mib, Some(&mut buffer), &mut size) {
            return Err(self.arguments_error(source));
        }

        buffer.truncate(size);

        Arguments::try_from(buffer.as_slice())
            .map_err(|error| os_error(io::Error::new(io::ErrorKind::InvalidData, error)))
    }

    /// The sysctl fails with `EINVAL` for a process that has exited, a process
    /// without arguments and another user's process. This function therefore
    /// calls [`Pid::info`] to choose between [`Error::Exited`],
    /// [`Error::Unsupported`] and [`Error::Denied`].
    fn arguments_error(self, source: io::Error) -> Error {
        if source.raw_os_error() != Some(libc::EINVAL) {
            return Error::for_process(Call::Arguments, self, source);
        }

        match self.info() {
            Err(error) => error,
            Ok(info) if self.as_raw() == 0 || info.status == Status::Zombie => Error::Unsupported {
                pid: self,
                call: Call::Arguments,
            },
            Ok(_) => Error::Denied {
                pid: self,
                call: Call::Arguments,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::PathBuf};

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Arguments, MalformedArguments};

    fn buffer(count: i32, rest: &[u8]) -> Vec<u8> {
        let mut buffer = count.to_ne_bytes().to_vec();
        buffer.extend_from_slice(rest);
        buffer
    }

    fn strings(strings: &[&str]) -> Vec<OsString> {
        strings.iter().map(OsString::from).collect()
    }

    #[rstest]
    #[case::own_process_with_apple_strings(
        buffer(2, b"/bin/ls\0ls\0-l\0HOME=/Users/a\0TERM=xterm\0\0\0\0ptr_munge=\0\0main_stack=\0"),
        Arguments {
            executable: PathBuf::from("/bin/ls"),
            arguments: strings(&["ls", "-l"]),
            environment: strings(&["HOME=/Users/a", "TERM=xterm"]),
        }
    )]
    #[case::environment_ending_on_a_boundary_before_apple_strings(
        buffer(1, b"/bin/ls\0ls\0HOME=/Users/abcdefgh\0pfz=0xffff40000\0stack_guard=0x1\0"),
        Arguments {
            executable: PathBuf::from("/bin/ls"),
            arguments: strings(&["ls"]),
            environment: strings(&["HOME=/Users/abcdefgh"]),
        }
    )]
    #[case::user_variable_named_pfz(
        buffer(1, b"/bin/ls\0ls\0pfz=1\0Z=2\0\0\0\0pfz=0xffff40000\0"),
        Arguments {
            executable: PathBuf::from("/bin/ls"),
            arguments: strings(&["ls"]),
            environment: strings(&["pfz=1", "Z=2"]),
        }
    )]
    #[case::user_variable_named_pfz_on_a_boundary(
        buffer(1, b"/bin/ls\0ls12345\0pfz=1\0Z=2\0\0\0\0\0\0\0pfz=0xffff40000\0"),
        Arguments {
            executable: PathBuf::from("/bin/ls"),
            arguments: strings(&["ls12345"]),
            environment: strings(&["pfz=1", "Z=2"]),
        }
    )]
    #[case::another_process_without_environment(
        buffer(3, b"/usr/bin/tail\0\0\0/usr/bin/tail\0-f\0/dev/null\0"),
        Arguments {
            executable: PathBuf::from("/usr/bin/tail"),
            arguments: strings(&["/usr/bin/tail", "-f", "/dev/null"]),
            environment: Vec::new(),
        }
    )]
    #[case::overwritten_arguments(
        buffer(3, b"/usr/bin/daemon\0daemon: worker process   \0"),
        Arguments {
            executable: PathBuf::from("/usr/bin/daemon"),
            arguments: strings(&["daemon: worker process   "]),
            environment: Vec::new(),
        }
    )]
    #[case::truncated_argument(
        buffer(2, b"/bin/echo\0\0\0\0\0\0\0echo\0hel"),
        Arguments {
            executable: PathBuf::from("/bin/echo"),
            arguments: strings(&["echo", "hel"]),
            environment: Vec::new(),
        }
    )]
    #[case::empty_first_argument(
        buffer(3, b"/bin/sh\0\0-c\0exit\0"),
        Arguments {
            executable: PathBuf::from("/bin/sh"),
            arguments: strings(&["", "-c", "exit"]),
            environment: Vec::new(),
        }
    )]
    #[case::cut_off_in_the_padding(
        buffer(1, b"/bin/echo\0\0\0"),
        Arguments {
            executable: PathBuf::from("/bin/echo"),
            arguments: Vec::new(),
            environment: Vec::new(),
        }
    )]
    #[case::only_the_count(buffer(0, b""), Arguments::default())]
    fn a_buffer_is_parsed(#[case] buffer: Vec<u8>, #[case] expected: Arguments) {
        assert_eq!(Arguments::try_from(buffer.as_slice()), Ok(expected));
    }

    #[rstest]
    #[case::too_short(b"ab".to_vec())]
    #[case::negative_count(buffer(-1, b"/bin/echo\0\0\0\0\0\0\0echo\0"))]
    fn a_buffer_without_a_valid_count_is_rejected(#[case] buffer: Vec<u8>) {
        assert_eq!(
            Arguments::try_from(buffer.as_slice()),
            Err(MalformedArguments)
        );
    }
}
