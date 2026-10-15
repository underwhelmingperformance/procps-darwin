// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2011-2023 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 2004-2023 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 1998-2002 Albert Cahalan
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated in part from procps-ng v4.0.7 src/ps/global.c.

use std::{
    ffi::{CString, OsStr, OsString},
    os::{fd::AsFd, unix::ffi::OsStrExt},
};

use darwin_proc::{CaseFolding, Gid, Locale, Uid};
use rustix::{
    fs::{FileType, Mode, OFlags},
    termios,
};

/// The type and device number of a file, from `stat`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileStatus {
    /// Whether the file is a character device.
    pub(crate) character_device: bool,
    /// The device that the file represents (`st_rdev`).
    pub(crate) device: libc::dev_t,
}

/// The size of a terminal window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowSize {
    /// The number of columns.
    pub(crate) columns: u16,
    /// The number of rows.
    pub(crate) rows: u16,
}

/// The letters of the locale's character encoding, which approximate the
/// characters that glibc's `isalpha` and `iswalpha` accept. procps-ng
/// classifies an argument with those functions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Letters {
    /// Whether the encoding is UTF-8.
    utf8: bool,
    /// Whether each byte, on its own, encodes a letter.
    bytes: [bool; 256],
}

impl Letters {
    /// The letters of the C locale, which are the ASCII letters.
    pub(crate) fn ascii() -> Self {
        let mut bytes = [false; 256];

        for (byte, letter) in (0..=u8::MAX).zip(&mut bytes) {
            *letter = byte.is_ascii_alphabetic();
        }

        Self { utf8: false, bytes }
    }

    /// The letters of the encoding that `nl_langinfo(CODESET)` calls
    /// `codeset`. In UTF-8 a single byte is a letter only if it is an ASCII
    /// letter. In another encoding a byte is a letter if the character that it
    /// encodes is alphabetic in Unicode, which approximates glibc's classes.
    pub(crate) fn for_codeset(codeset: &str) -> Self {
        let codeset = codeset
            .split_once('@')
            .map_or(codeset, |(codeset, _)| codeset);

        if codeset.eq_ignore_ascii_case("UTF-8") {
            return Self {
                utf8: true,
                ..Self::ascii()
            };
        }

        let decoded = CString::new(codeset)
            .ok()
            .and_then(|codeset| darwin_proc::decode_each_byte(&codeset));
        let Some(decoded) = decoded else {
            return Self::ascii();
        };
        let mut bytes = [false; 256];

        for (character, letter) in decoded.iter().zip(&mut bytes) {
            *letter = character.is_some_and(char::is_alphabetic);
        }

        Self { utf8: false, bytes }
    }

    /// Whether this approximation of glibc's `isalpha` classifies `byte` as a
    /// letter.
    pub(crate) fn is_letter(&self, byte: u8) -> bool {
        self.bytes[usize::from(byte)]
    }

    /// Whether `text` starts with a letter, which approximates glibc's
    /// `mbstowcs` followed by `iswalpha`. In UTF-8 the first character is
    /// decoded. In another encoding only the first byte is read, so the first
    /// byte of a character that has several bytes is not a letter.
    pub(crate) fn starts_with_letter(&self, text: &[u8]) -> bool {
        if !self.utf8 {
            return text.first().is_some_and(|&byte| self.is_letter(byte));
        }

        let prefix = &text[..text.len().min(4)];
        let valid = match std::str::from_utf8(prefix) {
            Ok(valid) => valid,
            Err(error) => std::str::from_utf8(&prefix[..error.valid_up_to()]).unwrap_or_default(),
        };

        valid.chars().next().is_some_and(char::is_alphabetic)
    }
}

/// What `ps`'s parser reads from the system, so tests can replace it.
pub(crate) trait System {
    /// The value of the environment variable `name`, as `getenv` returns it.
    fn variable(&self, name: &str) -> Option<OsString>;

    /// The ID of the user `name`, as `getpwnam` returns it.
    fn user(&self, name: &OsStr) -> Option<u32>;

    /// The ID of the group `name`, as `getgrnam` returns it.
    fn group(&self, name: &OsStr) -> Option<u32>;

    /// The status of the file at `path`, or `None` if `stat` fails.
    fn file(&self, path: &OsStr) -> Option<FileStatus>;

    /// The size of the first terminal of standard output, standard error,
    /// standard input and `/dev/tty` whose window has rows and columns.
    fn window_size(&self) -> Option<WindowSize>;

    /// Whether standard output is a terminal.
    fn stdout_is_terminal(&self) -> bool;

    /// The letters of the locale's character encoding, which approximate
    /// glibc's character classes.
    fn letters(&self) -> &Letters;

    /// How the locale folds case.
    fn case_folding(&self) -> CaseFolding;
}

/// The system that `ps` runs on.
pub(crate) struct Host {
    letters: Letters,
    case_folding: CaseFolding,
}

impl Host {
    /// The system, with the locale that the calling thread has.
    pub(crate) fn new() -> Self {
        let locale = Locale::current();

        Self {
            letters: Letters::for_codeset(locale.codeset()),
            case_folding: locale.case_folding(),
        }
    }
}

/// The size of the window of the terminal `fd`, if `fd` is a terminal and the
/// window has rows and columns.
fn window_of(fd: impl AsFd) -> Option<WindowSize> {
    let size = termios::tcgetwinsize(fd).ok()?;

    (size.ws_col > 0 && size.ws_row > 0).then_some(WindowSize {
        columns: size.ws_col,
        rows: size.ws_row,
    })
}

impl System for Host {
    fn variable(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn user(&self, name: &OsStr) -> Option<u32> {
        Uid::from_name(name).ok().flatten().map(u32::from)
    }

    fn group(&self, name: &OsStr) -> Option<u32> {
        Gid::from_name(name).ok().flatten().map(u32::from)
    }

    fn file(&self, path: &OsStr) -> Option<FileStatus> {
        let status = rustix::fs::stat(path.as_bytes()).ok()?;

        Some(FileStatus {
            character_device: FileType::from_raw_mode(status.st_mode) == FileType::CharacterDevice,
            device: status.st_rdev,
        })
    }

    fn window_size(&self) -> Option<WindowSize> {
        window_of(std::io::stdout())
            .or_else(|| window_of(std::io::stderr()))
            .or_else(|| window_of(std::io::stdin()))
            .or_else(|| {
                let flags = OFlags::NOCTTY | OFlags::NONBLOCK | OFlags::RDONLY;
                let terminal = rustix::fs::open("/dev/tty", flags, Mode::empty()).ok()?;

                window_of(terminal)
            })
    }

    fn stdout_is_terminal(&self) -> bool {
        termios::isatty(std::io::stdout())
    }

    fn letters(&self) -> &Letters {
        &self.letters
    }

    fn case_folding(&self) -> CaseFolding {
        self.case_folding
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{
        collections::HashMap,
        ffi::{OsStr, OsString},
    };

    use darwin_proc::CaseFolding;

    use super::{FileStatus, Letters, System, WindowSize};

    /// A system with fixed variables, users, groups and files, and no
    /// terminal.
    pub(crate) struct FakeSystem {
        pub(crate) variables: HashMap<&'static str, &'static str>,
        pub(crate) users: HashMap<&'static str, u32>,
        pub(crate) groups: HashMap<&'static str, u32>,
        pub(crate) files: HashMap<&'static str, FileStatus>,
        pub(crate) window: Option<WindowSize>,
        pub(crate) letters: Letters,
    }

    impl Default for FakeSystem {
        fn default() -> Self {
            Self {
                variables: HashMap::new(),
                users: HashMap::from([("root", 0), ("nobody", 4_294_967_294)]),
                groups: HashMap::from([("wheel", 0), ("staff", 20)]),
                files: HashMap::from([
                    (
                        "/dev/ttys001",
                        FileStatus {
                            character_device: true,
                            device: 0x1000_0001,
                        },
                    ),
                    (
                        "/dev/pts/3",
                        FileStatus {
                            character_device: true,
                            device: 0x1000_0003,
                        },
                    ),
                    (
                        "/dev/console",
                        FileStatus {
                            character_device: true,
                            device: 0x0000_0000,
                        },
                    ),
                    (
                        "/etc/passwd",
                        FileStatus {
                            character_device: false,
                            device: 0,
                        },
                    ),
                ]),
                window: None,
                letters: Letters::ascii(),
            }
        }
    }

    impl System for FakeSystem {
        fn variable(&self, name: &str) -> Option<OsString> {
            self.variables.get(name).map(OsString::from)
        }

        fn user(&self, name: &OsStr) -> Option<u32> {
            self.users.get(name.to_str()?).copied()
        }

        fn group(&self, name: &OsStr) -> Option<u32> {
            self.groups.get(name.to_str()?).copied()
        }

        fn file(&self, path: &OsStr) -> Option<FileStatus> {
            self.files.get(path.to_str()?).copied()
        }

        fn window_size(&self) -> Option<WindowSize> {
            self.window
        }

        fn stdout_is_terminal(&self) -> bool {
            self.window.is_some()
        }

        fn letters(&self) -> &Letters {
            &self.letters
        }

        fn case_folding(&self) -> CaseFolding {
            CaseFolding::Ascii
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::Letters;

    #[rstest]
    #[case::c("US-ASCII", b"\xe9", (false, false))]
    #[case::utf8_letter("UTF-8", "é".as_bytes(), (false, true))]
    #[case::utf8_digit("UTF-8", "٣".as_bytes(), (false, false))]
    #[case::utf8_invalid("UTF-8", b"\xff", (false, false))]
    #[case::latin1("ISO8859-1", b"\xe9", (true, true))]
    #[case::latin1_sign("ISO8859-1", b"\xd7", (false, false))]
    #[case::ascii_letter("US-ASCII", b"x", (true, true))]
    fn letters_follow_the_encoding(
        #[case] codeset: &str,
        #[case] text: &[u8],
        #[case] expected: (bool, bool),
    ) {
        let letters = Letters::for_codeset(codeset);

        assert_eq!(
            (letters.is_letter(text[0]), letters.starts_with_letter(text)),
            expected
        );
    }
}
