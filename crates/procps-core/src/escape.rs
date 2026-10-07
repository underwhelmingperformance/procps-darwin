// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
// SPDX-FileCopyrightText: 2002-2025 Craig Small <csmall@dropbear.xyz>
// SPDX-FileCopyrightText: 2011-2026 Jim Warner <james.warner@comcast.net>
// SPDX-FileCopyrightText: 1998-2010 Albert Cahalan
// SPDX-FileCopyrightText: 2010-2011 Jan Görig <jgorig@redhat.com>
// SPDX-FileCopyrightText: 1998 Michael K. Johnson
// SPDX-FileCopyrightText: 1996 Charles L. Blake
//
// SPDX-License-Identifier: GPL-3.0-or-later

// Translated from procps-ng v4.0.7 library/escape.c and library/readproc.c.
// Both files are LGPL-2.1-or-later, and section 3 of that licence allows a
// translation under GPL-3.0-or-later.

use std::{
    ffi::{CString, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
};

use darwin_proc::Status;

use crate::{Field, Process};

/// The character encoding of the locale, which decides how procps-ng escapes
/// text.
///
/// ```
/// use procps_core::Charset;
///
/// assert_eq!(Charset::from_codeset("utf-8"), Charset::Utf8);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Charset {
    /// UTF-8. Valid characters other than control and private-use characters
    /// are kept.
    Utf8,
    /// Any other encoding. The printable bytes are kept.
    Other(PrintableBytes),
}

impl Charset {
    /// The charset for `codeset`, which is the name that
    /// `nl_langinfo(CODESET)` reports. procps-ng compares the name with
    /// `UTF-8` without regard to case. For a locale with a modifier, such as
    /// `sr_RS.UTF-8@latin`, Darwin reports the codeset as `UTF-8@latin`, and
    /// glibc reports `UTF-8`. This function removes the modifier before it
    /// compares the codeset.
    ///
    /// ```
    /// use procps_core::Charset;
    ///
    /// assert_ne!(Charset::from_codeset("US-ASCII"), Charset::Utf8);
    /// ```
    #[must_use]
    pub fn from_codeset(codeset: &str) -> Self {
        let codeset = codeset
            .split_once('@')
            .map_or(codeset, |(codeset, _)| codeset);

        if codeset.eq_ignore_ascii_case("UTF-8") {
            return Self::Utf8;
        }

        Self::Other(PrintableBytes::for_codeset(codeset))
    }
}

/// The bytes that glibc's `isprint` accepts in an encoding other than UTF-8.
///
/// glibc's locales classify a character as printable unless it is a control
/// character, and a byte is printable if it encodes a printable character.
/// Darwin's `isprint` also accepts bytes that encode no character, such as
/// 0xae in ISO 8859-7, so the set comes from decoding each byte with `iconv`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrintableBytes([u64; 4]);

impl PrintableBytes {
    /// The printable bytes of `codeset`, or those of ASCII if `iconv` does
    /// not know `codeset`.
    fn for_codeset(codeset: &str) -> Self {
        let decoded = CString::new(codeset)
            .ok()
            .and_then(|codeset| darwin_proc::decode_each_byte(&codeset));
        let printable = |byte: u8| match &decoded {
            Some(decoded) => {
                decoded[usize::from(byte)].is_some_and(|character| !character.is_control())
            }
            None => byte.is_ascii_graphic() || byte == b' ',
        };

        let mut set = [0; 4];

        for byte in (0..=u8::MAX).filter(|&byte| printable(byte)) {
            set[usize::from(byte / 64)] |= 1 << (byte % 64);
        }

        Self(set)
    }

    const fn contains(self, byte: u8) -> bool {
        self.0[(byte / 64) as usize] & (1 << (byte % 64)) != 0
    }
}

/// The size of procps-ng's buffers for a command line or an environment
/// string. procps-ng keeps at most one byte fewer than this, because it
/// reserves the last byte for the terminating NUL.
const COMMAND_LINE_BUFFER: usize = 1024 * 64 * 2;

/// `text` as procps-ng's `escape_str` shows it: each byte of a character that
/// would be unsafe on a terminal, and each byte of an invalid sequence, is
/// replaced by `?`. Like a C string, `text` ends at its first NUL.
///
/// In UTF-8, procps-ng keeps valid characters and replaces control
/// characters, C1 control characters, private-use characters, and each byte
/// of an invalid sequence. In any other encoding, procps-ng keeps the bytes
/// that `isprint` accepts. In those encodings, the result can contain bytes
/// that are not UTF-8.
#[must_use]
fn escape(text: &[u8], charset: Charset) -> OsString {
    let end = text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len());
    let mut bytes = text[..end].to_vec();

    match charset {
        Charset::Utf8 => escape_utf8(&mut bytes),
        Charset::Other(printable) => {
            for byte in &mut bytes {
                if !printable.contains(*byte) {
                    *byte = b'?';
                }
            }
        }
    }

    OsString::from_vec(bytes)
}

/// How `escape_utf8` treats the sequence at the start of a text.
enum Step {
    /// Keep this many bytes.
    Keep(usize),
    /// Keep this many bytes, unless the sequence is a control character.
    KeepUnlessControl(usize),
    /// Replace the first byte.
    Escape,
}

/// Replaces the unsafe characters of `bytes` with `?`, as procps-ng's
/// `u8charlen` does.
fn escape_utf8(bytes: &mut [u8]) {
    let mut start = 0;

    while start < bytes.len() {
        let sequence = &bytes[start..];
        let byte = |index: usize| sequence.get(index).copied().unwrap_or(0);

        match step(sequence) {
            Step::Keep(length) => start += length,
            Step::KeepUnlessControl(length)
                if byte(0) >= 0x20
                    && byte(0) != 0x7f
                    && !(byte(0) == 0xc2 && (0x80..=0x9f).contains(&byte(1))) =>
            {
                start += length;
            }
            Step::KeepUnlessControl(_) | Step::Escape => {
                bytes[start] = b'?';
                start += 1;
            }
        }
    }
}

/// How procps-ng's `u8charlen` treats the UTF-8 sequence that `bytes` starts
/// with.
fn step(bytes: &[u8]) -> Step {
    let byte = |index: usize| u32::from(bytes[index]);
    let continues = |index: usize| bytes.get(index).is_some_and(|&byte| byte & 0xc0 == 0x80);

    if byte(0) <= 0x7f {
        return Step::KeepUnlessControl(1);
    }

    if !continues(1) {
        return Step::Escape;
    }

    if (0xc2..=0xdf).contains(&byte(0)) {
        return Step::KeepUnlessControl(2);
    }

    if !continues(2) {
        return Step::Escape;
    }

    // U+E000 to U+F8FF, the primary private-use area.
    let three = (byte(0) << 16) | (byte(1) << 8) | byte(2);

    if (0x00ee_8080..=0x00ef_a3bf).contains(&three) {
        return Step::Escape;
    }

    // The first two bytes decide whether the sequence is overlong or encodes a
    // surrogate.
    let leading = (byte(0) << 6) | (byte(1) & 0x3f);

    if (0x3820..=0x3b5f).contains(&leading) || (0x3b80..=0x3bff).contains(&leading) {
        return Step::Keep(3);
    }

    if !continues(3) {
        return Step::Escape;
    }

    // U+F0000 to U+FFFFD and U+100000 to U+10FFFD, the supplementary
    // private-use areas.
    let four = (three << 8) | byte(3);

    if (0xf3b0_8080..=0xf3bf_bfbd).contains(&four) || (0xf480_8080..=0xf48f_bfbd).contains(&four) {
        return Step::Escape;
    }

    if (0x3c10..=0x3d0f).contains(&leading) {
        return Step::Keep(4);
    }

    Step::Escape
}

impl Process {
    /// The command name as procps-ng's `CMD` item shows it: the first 15 bytes
    /// of `p_comm`, escaped. If the 15 bytes end inside a multi-byte
    /// character, each byte of the partial character becomes `?`, as on
    /// Linux.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Charset, Field, Process};
    ///
    /// let mut info = Pid::current().info()?;
    /// info.comm = "tab\tname".into();
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_command(Charset::Utf8), "tab?name");
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_command(&self, charset: Charset) -> OsString {
        escape(self.linux_comm(), charset)
    }

    /// The command line as procps-ng's `CMDLINE` item shows it.
    ///
    /// procps-ng joins the arguments with spaces, replaces newlines with
    /// spaces, and escapes the result. Empty arguments at the end are left
    /// out, and if every argument is empty, the command line is `-`. If the
    /// process has no arguments, or the source could not read them, the
    /// command line is the command name in brackets, followed by
    /// ` <defunct>` for a zombie. procps-ng shows the command line of a Linux
    /// kernel thread in the same way, because the thread has no arguments, and
    /// it does the same when it cannot read `/proc/<pid>/cmdline`.
    ///
    /// ```
    /// use darwin_proc::Pid;
    /// use procps_core::{Charset, Field, Process};
    ///
    /// let mut info = Pid::current().info()?;
    /// info.comm = "launchd".into();
    /// let process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    ///
    /// assert_eq!(process.linux_command_line(Charset::Utf8), "[launchd]");
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_command_line(&self, charset: Charset) -> OsString {
        let arguments = match &self.arguments {
            Some(Field::Available(arguments)) if !arguments.is_empty() => arguments,
            _ => return self.bracketed_command(charset),
        };

        let line = unvectored(arguments);

        if line.is_empty() {
            return OsString::from("-");
        }

        escape(&line, charset)
    }

    /// The command name in brackets, as procps-ng's `escape_command` shows
    /// it with `ESC_BRACKETS` and `ESC_DEFUNCT`.
    fn bracketed_command(&self, charset: Charset) -> OsString {
        let command = self.linux_command(charset);
        let defunct: &[u8] = if self.info.status == Status::Zombie {
            b" <defunct>"
        } else {
            b""
        };

        OsString::from_vec([b"[", command.as_bytes(), b"]", defunct].concat())
    }

    /// The environment as procps-ng's `ENVIRON_V` item gives it: one escaped
    /// string for each line of each variable, cut to 131071 bytes, or `None` if
    /// the source could not read the environment. For an empty or unreadable
    /// environment, procps-ng's item is `["-"]`. No entry of `pgrep --env`
    /// matches `-`, because an entry matches only a variable with `=`, so the
    /// empty list and `None` select the same processes.
    ///
    /// ```
    /// use std::ffi::OsString;
    ///
    /// use darwin_proc::Pid;
    /// use procps_core::{Charset, Field, Process};
    ///
    /// let info = Pid::current().info()?;
    /// let mut process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
    /// process.environment = Some(Field::Available(vec![OsString::from("A=1\nB")]));
    ///
    /// assert_eq!(
    ///     process.linux_environment(Charset::Utf8),
    ///     Some(vec![OsString::from("A=1"), OsString::from("B")])
    /// );
    /// # Ok::<(), darwin_proc::Error>(())
    /// ```
    #[must_use]
    pub fn linux_environment(&self, charset: Charset) -> Option<Vec<OsString>> {
        let Some(Field::Available(environment)) = &self.environment else {
            return None;
        };

        Some(
            environment
                .iter()
                .flat_map(|variable| variable.as_bytes().split(|&byte| byte == b'\n'))
                .map(|line| escape(&line[..line.len().min(COMMAND_LINE_BUFFER - 1)], charset))
                .collect(),
        )
    }
}

/// The arguments as procps-ng's `read_unvectored` reads `/proc/<pid>/cmdline`
/// with spaces as separators. procps-ng's caller escapes the result
/// afterwards.
fn unvectored(arguments: &[OsString]) -> Vec<u8> {
    let mut line: Vec<u8> = arguments
        .iter()
        .flat_map(|argument| argument.as_bytes().iter().copied().chain([0]))
        .collect();
    line.truncate(COMMAND_LINE_BUFFER - 1);

    let length = line.len();
    let end = line
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |last| last + 1);

    for byte in &mut line[..end] {
        if *byte == b'\n' || *byte == 0 {
            *byte = b' ';
        }
    }

    if line.get(length.wrapping_sub(1)) == Some(&b' ') {
        line[length - 1] = 0;
    }

    line.truncate(
        line.iter()
            .position(|&byte| byte == 0)
            .unwrap_or(line.len()),
    );

    line
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    use darwin_proc::{Pid, Status};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{Charset, escape};
    use crate::{Field, Process};

    #[rstest]
    #[case::ascii(b"sshd", "sshd")]
    #[case::control(b"a\tb\x7f", "a?b?")]
    #[case::two_bytes("caf\u{e9}".as_bytes(), "caf\u{e9}")]
    #[case::c1_control(b"\xc2\x85", "??")]
    #[case::last_c1_control("\u{9f}".as_bytes(), "??")]
    #[case::after_the_c1_controls("\u{a0}".as_bytes(), "\u{a0}")]
    #[case::invalid(b"\xff", "?")]
    #[case::overlong_two_bytes(b"\xc0\x80", "??")]
    #[case::truncated(b"a\xc3", "a?")]
    #[case::three_bytes("\u{20ac}".as_bytes(), "\u{20ac}")]
    #[case::surrogate(b"\xed\xa0\x80", "???")]
    #[case::overlong(b"\xe0\x80\x80", "???")]
    #[case::private_use("\u{e000}".as_bytes(), "???")]
    #[case::last_private_use("\u{f8ff}".as_bytes(), "???")]
    #[case::replacement_character("\u{fffd}".as_bytes(), "\u{fffd}")]
    #[case::four_bytes("\u{1f600}".as_bytes(), "\u{1f600}")]
    #[case::supplementary_private_use("\u{f0000}".as_bytes(), "????")]
    #[case::last_supplementary_private_use("\u{ffffd}".as_bytes(), "????")]
    #[case::after_the_supplementary_private_use("\u{ffffe}".as_bytes(), "\u{ffffe}")]
    #[case::second_supplementary_private_use("\u{100000}".as_bytes(), "????")]
    #[case::last_second_supplementary_private_use("\u{10fffd}".as_bytes(), "????")]
    #[case::last_character("\u{10ffff}".as_bytes(), "\u{10ffff}")]
    #[case::above_unicode(b"\xf4\x90\x80\x80", "????")]
    #[case::nul(b"a\0b", "a")]
    fn utf8_is_escaped_as_procps_ng_escapes_it(#[case] text: &[u8], #[case] expected: &str) {
        assert_eq!(escape(text, Charset::Utf8), expected);
    }

    #[rstest]
    #[case::ascii("US-ASCII", b"sshd -D", b"sshd -D")]
    #[case::utf8_in_ascii("US-ASCII", "caf\u{e9}".as_bytes(), b"caf??")]
    #[case::control("US-ASCII", b"a\tb\x7f", b"a?b?")]
    #[case::latin_letter("ISO8859-1", b"caf\xe9", b"caf\xe9")]
    #[case::latin_symbol("ISO8859-1", b"\xa0\xa9\xd7", b"\xa0\xa9\xd7")]
    #[case::c1_control("ISO8859-1", b"\x85", b"?")]
    #[case::unassigned("ISO8859-3", b"\xa5", b"?")]
    #[case::unassigned_that_darwin_prints("ISO8859-7", b"\xae", b"?")]
    #[case::cyrillic("KOI8-R", b"\xc1", b"\xc1")]
    #[case::unknown_codeset("NO-SUCH-CODESET", b"a \xe9", b"a ?")]
    #[case::utf8_with_a_modifier("UTF-8@latin", "caf\u{e9}".as_bytes(), "caf\u{e9}".as_bytes())]
    #[case::latin_with_a_modifier("ISO8859-15@euro", b"caf\xe9", b"caf\xe9")]
    fn other_charsets_keep_the_bytes_that_glibc_prints(
        #[case] codeset: &str,
        #[case] text: &[u8],
        #[case] expected: &[u8],
    ) {
        assert_eq!(
            escape(text, Charset::from_codeset(codeset)),
            OsString::from_vec(expected.to_vec())
        );
    }

    fn process(
        comm: &str,
        status: Status,
        arguments: Option<Field<Vec<OsString>>>,
    ) -> Result<Process, darwin_proc::Error> {
        let mut info = Pid::current().info()?;
        info.comm = comm.into();
        info.status = status;
        let mut process = Process::from_identity(info, Field::Unsupported, Field::Unsupported);
        process.arguments = arguments;

        Ok(process)
    }

    #[rstest]
    #[case::joined(Some(Field::Available(vec!["sleep", "10"])), Status::Running, "sleep 10")]
    #[case::newline(Some(Field::Available(vec!["sh", "-c", "a\nb"])), Status::Running, "sh -c a b")]
    #[case::empty_inside(Some(Field::Available(vec!["a", "", "b"])), Status::Running, "a  b")]
    #[case::empty_at_end(Some(Field::Available(vec!["a", "", ""])), Status::Running, "a")]
    #[case::all_empty(Some(Field::Available(vec!["", ""])), Status::Running, "-")]
    #[case::none(Some(Field::Available(vec![])), Status::Running, "[worker]")]
    #[case::denied(Some(Field::Denied), Status::Running, "[worker]")]
    #[case::not_requested(None, Status::Running, "[worker]")]
    #[case::zombie(Some(Field::Unsupported), Status::Zombie, "[worker] <defunct>")]
    #[case::escaped(Some(Field::Available(vec!["a\tb"])), Status::Running, "a?b")]
    fn the_command_line_is_procps_ng_s(
        #[case] arguments: Option<Field<Vec<&str>>>,
        #[case] status: Status,
        #[case] expected: &str,
    ) -> Result<(), darwin_proc::Error> {
        let arguments = arguments.map(|field| {
            field.map(|arguments| arguments.into_iter().map(OsString::from).collect())
        });
        let process = process("worker", status, arguments)?;

        assert_eq!(process.linux_command_line(Charset::Utf8), expected);

        Ok(())
    }

    // procps-ng drops the last byte of a cut command line if it is a space,
    // including a newline that it has turned into a space.
    #[rstest]
    #[case::plain("x".repeat(200_000), "x".repeat(131_071))]
    #[case::newline_at_the_cut(format!("{}\ny", "x".repeat(131_070)), "x".repeat(131_070))]
    fn a_long_command_line_is_cut_where_procps_ng_cuts_it(
        #[case] argument: String,
        #[case] expected: String,
    ) -> Result<(), darwin_proc::Error> {
        let process = process(
            "worker",
            Status::Running,
            Some(Field::Available(vec![OsString::from(argument)])),
        )?;

        assert_eq!(
            process.linux_command_line(Charset::Utf8),
            OsString::from(expected)
        );

        Ok(())
    }

    #[rstest]
    #[case::partial_character("abcdefghijklmn\u{e9}", "abcdefghijklmn?")]
    #[case::partial_three_bytes("abcdefghijklm\u{20ac}x", "abcdefghijklm??")]
    #[case::whole_character("abcdefghijklm\u{e9}", "abcdefghijklm\u{e9}")]
    fn the_command_name_keeps_linux_s_15_bytes(
        #[case] comm: &str,
        #[case] expected: &str,
    ) -> Result<(), darwin_proc::Error> {
        let process = process(comm, Status::Running, None)?;

        assert_eq!(process.linux_command(Charset::Utf8), expected);

        Ok(())
    }

    #[test]
    fn a_long_variable_is_cut_where_procps_ng_cuts_it() -> Result<(), darwin_proc::Error> {
        let variable = format!("A={}", "x".repeat(200_000));
        let mut process = process("worker", Status::Running, None)?;
        process.environment = Some(Field::Available(vec![OsString::from(&variable)]));

        assert_eq!(
            process.linux_environment(Charset::Utf8),
            Some(vec![OsString::from(&variable[..131_071])])
        );

        Ok(())
    }

    #[test]
    fn a_variable_is_escaped_for_the_charset() -> Result<(), darwin_proc::Error> {
        let mut process = process("worker", Status::Running, None)?;
        process.environment = Some(Field::Available(vec![OsString::from("V=caf\u{e9}")]));

        assert_eq!(
            process.linux_environment(Charset::from_codeset("US-ASCII")),
            Some(vec![OsString::from("V=caf??")])
        );

        Ok(())
    }
}
