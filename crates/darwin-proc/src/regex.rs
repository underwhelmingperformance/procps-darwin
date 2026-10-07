// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    ffi::{CStr, CString, OsStr},
    marker::PhantomData,
    mem::MaybeUninit,
    os::unix::ffi::OsStrExt,
};

use libc::{c_char, c_int};

use crate::{CaseFolding, Locale, locale::OwnedLocale};

/// Storage for glibc's `regex_t`. Rust never reads the fields, which
/// `regcomp` initialises. A test compares this size and alignment with the
/// values that `c/regex_abi.c` exports from glibc's `regex.h`.
///
/// The fields point to memory that `regcomp` allocates, and never into the
/// struct itself, so a compiled expression can move. The compiler derives
/// `Send` and `Sync` for [`Regex`] from the array of bytes and from
/// `OwnedLocale`'s implementations of them. `regexec` adds
/// states to the compiled expression while it matches. Concurrent matches are
/// still sound, because `regexec` first locks a mutex in that allocated memory.
/// The mutex exists because `glibc-regex/config.h` defines `GNULIB_LOCK`, and
/// `regex_internal.h` does not compile without it.
#[repr(C, align(8))]
struct GlibcRegex([u8; 64]);

/// glibc's `regcomp` flags and error codes, some of which differ from Darwin's.
/// A test compares them with the values that `c/regex_abi.c` exports from
/// glibc's `regex.h`.
mod glibc {
    use libc::c_int;

    pub(super) const REG_EXTENDED: c_int = 1;
    pub(super) const REG_ICASE: c_int = 2;
    pub(super) const REG_NOSUB: c_int = 8;

    pub(super) const REG_BADPAT: c_int = 2;
    pub(super) const REG_ECOLLATE: c_int = 3;
    pub(super) const REG_ECTYPE: c_int = 4;
    pub(super) const REG_EESCAPE: c_int = 5;
    pub(super) const REG_ESUBREG: c_int = 6;
    pub(super) const REG_EBRACK: c_int = 7;
    pub(super) const REG_EPAREN: c_int = 8;
    pub(super) const REG_EBRACE: c_int = 9;
    pub(super) const REG_BADBR: c_int = 10;
    pub(super) const REG_ERANGE: c_int = 11;
    pub(super) const REG_ESPACE: c_int = 12;
    pub(super) const REG_BADRPT: c_int = 13;
    pub(super) const REG_EEND: c_int = 14;
    pub(super) const REG_ESIZE: c_int = 15;
}

// gnulib's `config.h` renames glibc's functions with the prefix `rpl_`, so
// they do not clash with Darwin's.
unsafe extern "C" {
    fn rpl_regcomp(preg: *mut GlibcRegex, pattern: *const c_char, cflags: c_int) -> c_int;
    fn rpl_regexec(
        preg: *const GlibcRegex,
        string: *const c_char,
        nmatch: usize,
        pmatch: *mut libc::c_void,
        eflags: c_int,
    ) -> c_int;
    fn rpl_regerror(
        errcode: c_int,
        preg: *const GlibcRegex,
        errbuf: *mut c_char,
        errbuf_size: usize,
    ) -> usize;
    fn rpl_regfree(preg: *mut GlibcRegex);
}

// `c/case.c` folds case for the regular expression code.
unsafe extern "C" {
    fn procps_set_case_folding(folding: c_int) -> c_int;
}

/// A POSIX extended regular expression, compiled and matched by glibc
/// 2.44's regular expression code, which this crate builds from
/// `glibc-regex`.
///
/// procps-ng compiles patterns with glibc's `regcomp` and matches them with
/// `regexec`. Darwin's library reads some patterns differently: its extended
/// syntax lacks glibc's extensions, such as back-references and `\w`, and
/// its enhanced syntax gives escapes such as `\d` meanings that glibc lacks.
/// Darwin's engine also misses matches for patterns such as `(^|\<)sshd`.
/// Building glibc's code gives glibc's syntax and error messages. Matching
/// still depends on the locale data: outside glibc, the code uses Darwin's
/// character classes and case tables, and it has no collation rules, so some
/// matches differ from glibc's. `docs/mappings.md` lists the differences.
///
/// `regcomp` reads the pattern in the calling thread's `LC_CTYPE` locale, and
/// it reads the name of the thread's `LC_COLLATE` locale for ranges, so call
/// [`set_locale`](crate::set_locale) before [`Regex::new`]. The expression
/// keeps a copy of that locale, and matches in it on any thread. With
/// `ignore_case`, it folds case with Darwin's tables, except for `i`, `I`, `İ`
/// and `ı` in a Turkish locale. It folds those four letters as glibc does, and
/// glibc does not pair `i` with `I` in a Turkish locale.
///
/// ```
/// use darwin_proc::Regex;
///
/// let regex = Regex::new("^ss(h|hd)$".as_ref(), false)?;
///
/// assert!(regex.is_match("sshd".as_ref()) && !regex.is_match("bash".as_ref()));
/// # Ok::<(), darwin_proc::RegexError>(())
/// ```
pub struct Regex {
    compiled: GlibcRegex,
    locale: OwnedLocale,
    case_folding: CaseFolding,
}

impl std::fmt::Debug for Regex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Regex").finish_non_exhaustive()
    }
}

/// The reason that glibc's `regcomp` rejected a pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegexErrorKind {
    /// `REG_BADPAT`: the pattern is not valid. glibc reports a pattern that
    /// ends straight after `[` or `[^` with this code.
    BadPattern,
    /// `REG_ECOLLATE`: a collating element is not valid.
    Collation,
    /// `REG_ECTYPE`: a character class is unknown.
    CharacterClass,
    /// `REG_EESCAPE`: the pattern ends with a backslash.
    TrailingBackslash,
    /// `REG_ESUBREG`: a back-reference refers to a group that does not exist.
    BackReference,
    /// `REG_EBRACK`: a bracket expression has no closing bracket.
    Bracket,
    /// `REG_EPAREN`: an opening parenthesis is unmatched. In an extended
    /// expression, an unmatched closing parenthesis is an ordinary character.
    Parenthesis,
    /// `REG_EBRACE`: a brace is unmatched.
    Brace,
    /// `REG_BADBR`: the contents of an interval are not valid.
    Interval,
    /// `REG_ERANGE`: a range in a bracket expression is not valid.
    Range,
    /// `REG_ESPACE`: `regcomp` ran out of memory.
    Memory,
    /// `REG_BADRPT`: a repetition operator has no operand.
    Repetition,
    /// `REG_EEND`: the pattern ends too early.
    End,
    /// `REG_ESIZE`: the compiled expression would be too big.
    Size,
    /// A code that glibc's `regcomp` does not return, such as `REG_ERPAREN`,
    /// which `regcomp` reports as `REG_EPAREN`.
    Other,
}

impl RegexErrorKind {
    const fn from_code(code: c_int) -> Self {
        match code {
            glibc::REG_BADPAT => Self::BadPattern,
            glibc::REG_ECOLLATE => Self::Collation,
            glibc::REG_ECTYPE => Self::CharacterClass,
            glibc::REG_EESCAPE => Self::TrailingBackslash,
            glibc::REG_ESUBREG => Self::BackReference,
            glibc::REG_EBRACK => Self::Bracket,
            glibc::REG_EPAREN => Self::Parenthesis,
            glibc::REG_EBRACE => Self::Brace,
            glibc::REG_BADBR => Self::Interval,
            glibc::REG_ERANGE => Self::Range,
            glibc::REG_ESPACE => Self::Memory,
            glibc::REG_BADRPT => Self::Repetition,
            glibc::REG_EEND => Self::End,
            glibc::REG_ESIZE => Self::Size,
            _ => Self::Other,
        }
    }
}

/// A pattern that `regcomp` rejected.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RegexError {
    /// The kind of error.
    pub kind: RegexErrorKind,
    /// glibc's message from `regerror`, such as `Unmatched ( or \(`.
    pub message: String,
}

/// The bytes of `text` before its first NUL, which is where a C string ends.
fn c_string(text: &OsStr) -> CString {
    let bytes = text.as_bytes();
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());

    CString::new(&bytes[..end]).unwrap_or_default()
}

impl Regex {
    /// Compiles `pattern` as procps-ng does, with `REG_EXTENDED` and
    /// `REG_NOSUB`, so that matching reports only whether the text matches.
    /// With `ignore_case`, the expression matches without regard to case. A
    /// pattern with a NUL byte ends at that byte, where a C string ends.
    ///
    /// ```
    /// use darwin_proc::Regex;
    ///
    /// assert!(Regex::new("SSHD".as_ref(), true)?.is_match("sshd".as_ref()));
    /// # Ok::<(), darwin_proc::RegexError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`RegexError`] with glibc's message if `regcomp` rejects the
    /// pattern, or with the kind [`RegexErrorKind::Memory`] if the locale
    /// cannot be copied.
    pub fn new(pattern: &OsStr, ignore_case: bool) -> Result<Self, RegexError> {
        let locale = OwnedLocale::current().ok_or_else(|| RegexError {
            kind: RegexErrorKind::Memory,
            message: error_message(glibc::REG_ESPACE),
        })?;
        let case_folding = Locale::current().case_folding();
        let pattern = c_string(pattern);
        let case = if ignore_case { glibc::REG_ICASE } else { 0 };
        let flags = glibc::REG_EXTENDED | glibc::REG_NOSUB | case;
        let mut compiled = MaybeUninit::<GlibcRegex>::uninit();

        let code = {
            let _folding = Folding::enter(case_folding);

            // SAFETY: `compiled` is writable storage with the size and
            // alignment of glibc's `regex_t`, and `pattern` is a
            // NUL-terminated string that outlives the call.
            unsafe { rpl_regcomp(compiled.as_mut_ptr(), pattern.as_ptr(), flags) }
        };

        // A failed `regcomp` frees the memory that it allocated, so `compiled`
        // needs no `regfree`.
        if code != 0 {
            return Err(RegexError {
                kind: RegexErrorKind::from_code(code),
                message: error_message(code),
            });
        }

        // SAFETY: `regcomp` returned 0, so it initialised `compiled`. The
        // expression can move out of `compiled` because no field points into
        // the struct.
        let compiled = unsafe { compiled.assume_init() };

        Ok(Self {
            compiled,
            locale,
            case_folding,
        })
    }

    /// Whether the expression matches somewhere in `text`. A text with a NUL
    /// byte is matched up to that byte, where a C string ends.
    ///
    /// ```
    /// use darwin_proc::Regex;
    ///
    /// assert!(Regex::new("[[:digit:]]+".as_ref(), false)?.is_match("a1".as_ref()));
    /// # Ok::<(), darwin_proc::RegexError>(())
    /// ```
    #[must_use]
    pub fn is_match(&self, text: &OsStr) -> bool {
        let text = c_string(text);
        let _locale = self.locale.enter();
        let _folding = Folding::enter(self.case_folding);

        // SAFETY: `compiled` contains an expression that `regcomp` compiled
        // with `REG_NOSUB`, so the call writes no matches, and `text` is a
        // NUL-terminated string that outlives the call.
        let code = unsafe {
            rpl_regexec(
                &raw const self.compiled,
                text.as_ptr(),
                0,
                std::ptr::null_mut(),
                0,
            )
        };

        code == 0
    }
}

/// The case folding of the regular expression code on the calling thread,
/// until the guard is dropped, which restores the previous folding. The guard
/// is not `Send`, because the folding belongs to the thread.
struct Folding {
    previous: c_int,
    thread: PhantomData<*const ()>,
}

impl Folding {
    fn enter(folding: CaseFolding) -> Self {
        let value = match folding {
            CaseFolding::Ascii => 0,
            CaseFolding::TurkishUtf8 => 1,
            CaseFolding::TurkishLatin5 => 2,
        };

        // SAFETY: the function only sets a thread-local variable and returns
        // its previous value.
        let previous = unsafe { procps_set_case_folding(value) };

        Self {
            previous,
            thread: PhantomData,
        }
    }
}

impl Drop for Folding {
    fn drop(&mut self) {
        // SAFETY: as in `Folding::enter`.
        unsafe { procps_set_case_folding(self.previous) };
    }
}

impl Drop for Regex {
    fn drop(&mut self) {
        // SAFETY: `compiled` contains an expression that `regcomp` compiled,
        // and nothing uses it after this call.
        unsafe { rpl_regfree(&raw mut self.compiled) };
    }
}

/// glibc's message for the error code `code`.
fn error_message(code: c_int) -> String {
    let mut buffer = [0_u8; 256];

    // SAFETY: `buffer` has `buffer.len()` writable bytes, and `regerror`
    // writes a NUL-terminated message of at most that many bytes. After a
    // failed `regcomp`, the expression is not known to be initialised, so this
    // call passes a null pointer. glibc's `regerror` reads only the code.
    unsafe {
        rpl_regerror(
            code,
            std::ptr::null(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        );
    }

    CStr::from_bytes_until_nul(&buffer)
        .map(|message| message.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use libc::c_int;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{GlibcRegex, Regex, RegexErrorKind, glibc};

    /// The values of glibc's `regex.h` that `c/regex_abi.c` exports.
    #[repr(C)]
    #[derive(Debug, Eq, PartialEq)]
    struct RegexAbi {
        size: usize,
        alignment: usize,
        flags: [c_int; 3],
        errors: [c_int; 14],
    }

    unsafe extern "C" {
        static procps_regex_abi: RegexAbi;
    }

    #[test]
    fn the_declarations_match_glibc_s_regex_h() {
        // SAFETY: `c/regex_abi.c` defines `procps_regex_abi` as a constant
        // with this layout.
        let abi = unsafe { &procps_regex_abi };

        assert_eq!(
            *abi,
            RegexAbi {
                size: size_of::<GlibcRegex>(),
                alignment: align_of::<GlibcRegex>(),
                flags: [glibc::REG_EXTENDED, glibc::REG_ICASE, glibc::REG_NOSUB],
                errors: [
                    glibc::REG_BADPAT,
                    glibc::REG_ECOLLATE,
                    glibc::REG_ECTYPE,
                    glibc::REG_EESCAPE,
                    glibc::REG_ESUBREG,
                    glibc::REG_EBRACK,
                    glibc::REG_EPAREN,
                    glibc::REG_EBRACE,
                    glibc::REG_BADBR,
                    glibc::REG_ERANGE,
                    glibc::REG_ESPACE,
                    glibc::REG_BADRPT,
                    glibc::REG_EEND,
                    glibc::REG_ESIZE,
                ],
            }
        );
    }

    #[rstest]
    #[case::alternation("ss(h|hd)", "sshd", true)]
    #[case::anchored("^h$", "sh", false)]
    #[case::bracket("[]a]", "]", true)]
    #[case::class("[[:upper:]]", "abc", false)]
    #[case::back_reference("^(a)\\1$", "aa", true)]
    #[case::space(r"a\sb", "a b", true)]
    #[case::word_boundary(r"\bfoo\b", "a foo b", true)]
    #[case::interval("^a{2}$", "aa", true)]
    #[case::word_start_in_alternation(r"(^|\<)sshd", "/usr/sbin/sshd -D", true)]
    #[case::word_end_in_alternation(r"sshd($|\>)", "/usr/sbin/sshd -D", true)]
    #[case::not_a_boundary_at_the_start(r"\B-zsh", "-zsh", true)]
    #[case::optional_interval("x{1}?$", "abc", true)]
    #[case::escaped_letter(r"\d", "d", true)]
    #[case::buffer_anchors(r"\`a.\'", "ab", true)]
    #[case::empty("", "abc", true)]
    #[case::empty_alternative("sshd|", "bash", true)]
    #[case::nul_ends_the_pattern("a\0b", "a", true)]
    #[case::nul_leaves_the_start("b\0a", "a", false)]
    #[case::byte_in_the_c_locale("^.$", "\u{e9}", false)]
    fn an_expression_matches_as_glibc_matches_it(
        #[case] pattern: &str,
        #[case] text: &str,
        #[case] matches: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            Regex::new(pattern.as_ref(), false)?.is_match(text.as_ref()),
            matches
        );

        Ok(())
    }

    #[rstest]
    #[case::letters("SSHD", "sshd", true)]
    #[case::bracket("^[a-c]$", "B", true)]
    #[case::escaped_letter(r"SSH\d", "sshd", false)]
    fn ignore_case_folds_the_text_and_unescaped_letters_to_upper_case(
        #[case] pattern: &str,
        #[case] text: &str,
        #[case] matches: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            Regex::new(pattern.as_ref(), true)?.is_match(text.as_ref()),
            matches
        );

        Ok(())
    }

    #[rstest]
    #[case::bracket("[a", RegexErrorKind::Bracket, "Unmatched [, [^, [:, [., or [=")]
    #[case::parenthesis("(", RegexErrorKind::Parenthesis, r"Unmatched ( or \(")]
    #[case::brace("a{1", RegexErrorKind::Brace, r"Unmatched \{")]
    #[case::interval("a{2,1}", RegexErrorKind::Interval, r"Invalid content of \{\}")]
    #[case::class(
        "[[:bogus:]]",
        RegexErrorKind::CharacterClass,
        "Invalid character class name"
    )]
    #[case::darwin_class(
        "[[:rune:]]",
        RegexErrorKind::CharacterClass,
        "Invalid character class name"
    )]
    #[case::range("[z-a]", RegexErrorKind::Range, "Invalid range end")]
    #[case::backslash("a\\", RegexErrorKind::TrailingBackslash, "Trailing backslash")]
    #[case::back_reference(r"(a)b|\2", RegexErrorKind::BackReference, "Invalid back reference")]
    #[case::repetition(
        "(?i)a",
        RegexErrorKind::Repetition,
        "Invalid preceding regular expression"
    )]
    #[case::size("a{32768}", RegexErrorKind::Size, "Regular expression too big")]
    #[case::bracket_at_end("[", RegexErrorKind::BadPattern, "Invalid regular expression")]
    fn an_invalid_pattern_has_glibc_s_error(
        #[case] pattern: &str,
        #[case] kind: RegexErrorKind,
        #[case] message: &str,
    ) {
        let error = Regex::new(pattern.as_ref(), false)
            .map(|_| ())
            .map_err(|error| (error.kind, error.message));

        assert_eq!(error, Err((kind, message.to_owned())));
    }
}
