// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sets the locale of the calling thread.

use std::{
    ffi::{CStr, OsStr},
    io::Write,
    os::unix::ffi::OsStrExt,
    process::{Command, Stdio},
    thread,
};

use darwin_proc::{CaseFolding, Locale, Regex};
use pretty_assertions::assert_eq;
use rstest::rstest;

#[rstest]
#[case::c(c"C", Some("US-ASCII"))]
#[case::utf8(c"en_US.UTF-8", Some("UTF-8"))]
#[case::unknown(c"xx_XX.NOPE", None)]
#[case::linux_spelling(c"C.utf8", Some("UTF-8"))]
#[case::linux_spelling_with_a_language(c"en_US.utf8", Some("UTF-8"))]
#[case::glibc_spelling_of_a_codeset(c"en_US.ISO-8859-1", Some("ISO8859-1"))]
#[case::no_codeset(c"en_US", Some("ISO8859-1"))]
#[case::no_codeset_in_utf8(c"en_IN", Some("UTF-8"))]
#[case::locale_that_glibc_lacks(c"sr_YU", None)]
#[case::composite_without_a_codeset(c"C/en_US/C/C/C/C", Some("ISO8859-1"))]
fn a_locale_has_a_codeset(#[case] name: &CStr, #[case] expected: Option<&str>) {
    assert_eq!(
        darwin_proc::set_locale(name).map(|locale| locale.codeset().to_owned()),
        expected.map(str::to_owned)
    );
}

#[rstest]
#[case::c(c"C", CaseFolding::Ascii)]
#[case::english(c"en_US.UTF-8", CaseFolding::Ascii)]
#[case::turkish_utf8(c"tr_TR.UTF-8", CaseFolding::TurkishUtf8)]
#[case::turkish_latin5(c"tr_TR.ISO-8859-9", CaseFolding::TurkishLatin5)]
#[case::turkish_without_a_codeset(c"tr_TR", CaseFolding::TurkishLatin5)]
#[case::turkish_collation_only(c"tr_TR.UTF-8/C/C/C/C/C", CaseFolding::Ascii)]
fn a_locale_folds_case_as_glibc_folds_it(#[case] name: &CStr, #[case] expected: CaseFolding) {
    assert_eq!(
        darwin_proc::set_locale(name).map(|locale| locale.case_folding()),
        Some(expected)
    );
}

// glibc's Turkish locales do not pair `i` with `I`, so a pattern with `I` does
// not match `i` under `-i`. In UTF-8, `i` pairs with `İ`. In ISO 8859-9, `i`
// pairs with the byte 0xDD, and `I` with the byte 0xFD.
#[rstest]
#[case::capital_i_in_utf8(c"tr_TR.UTF-8", b"^INIX$", b"inix", false)]
#[case::mixed_case_in_utf8(c"tr_TR.UTF-8", b"^Inix$", b"inix", false)]
#[case::bracket_in_utf8(c"tr_TR.UTF-8", b"^IN[I]X$", b"inix", false)]
#[case::small_i_in_utf8(c"tr_TR.UTF-8", b"^inix$", b"inix", true)]
#[case::dotted_capital_i_in_utf8(c"tr_TR.UTF-8", "^\u{130}NX$".as_bytes(), b"inx", true)]
#[case::capital_i_in_latin5(c"tr_TR.ISO8859-9", b"^INIX$", b"inix", false)]
#[case::dotted_capital_i_in_latin5(c"tr_TR.ISO8859-9", b"^\xddNX$", b"inx", true)]
#[case::dotless_small_i_in_latin5(c"tr_TR.ISO8859-9", b"^\xfdNX$", b"INX", true)]
#[case::dotless_small_i_in_latin5_text(c"tr_TR.ISO8859-9", b"^INX$", b"\xfdnx", true)]
#[case::capital_i_in_english(c"en_US.UTF-8", b"^INIX$", b"inix", true)]
fn a_pattern_folds_case_as_glibc_folds_it(
    #[case] locale: &CStr,
    #[case] pattern: &[u8],
    #[case] text: &[u8],
    #[case] matches: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    darwin_proc::set_locale(locale).ok_or("the locale exists")?;

    let regex = Regex::new(OsStr::from_bytes(pattern), true).map_err(|error| error.message)?;

    assert_eq!(regex.is_match(OsStr::from_bytes(text)), matches);

    Ok(())
}

#[test]
fn darwin_installs_none_of_glibc_s_other_locales_with_their_own_case_rules() -> std::io::Result<()>
{
    let languages = ["az_AZ", "crh_UA", "ku_TR", "mt_MT", "tr_CY", "tt_RU"];
    let mut installed: Vec<String> = std::fs::read_dir("/usr/share/locale")?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| languages.iter().any(|language| name.starts_with(language)))
        .collect();
    installed.sort();

    assert_eq!(installed, Vec::<String>::new());

    Ok(())
}

// Darwin's loader for every category except `LC_CTYPE` keeps the data that it
// loaded last in a static cache without a lock, so `set_locale` serialises its
// calls to `newlocale`. This test calls `set_locale` on several threads at
// once.
#[test]
fn threads_can_set_their_locales_at_the_same_time() -> Result<(), Box<dyn std::error::Error>> {
    const NAMES: [(&CStr, &str); 4] = [
        (c"en_US.UTF-8", "UTF-8"),
        (c"fr_FR.ISO8859-1", "ISO8859-1"),
        (c"de_DE.ISO8859-1", "ISO8859-1"),
        (
            c"en_GB.UTF-8/C/en_US.UTF-8/de_DE.UTF-8/fr_FR.UTF-8/C",
            "US-ASCII",
        ),
    ];
    let set = |thread: usize| -> Vec<Option<String>> {
        (0..200)
            .map(|call| {
                let (name, _) = NAMES[(thread + call) % NAMES.len()];

                darwin_proc::set_locale(name).map(|locale| locale.codeset().to_owned())
            })
            .collect()
    };
    let expected: Vec<Vec<Option<String>>> = (0..8)
        .map(|thread| {
            (0..200)
                .map(|call| Some(NAMES[(thread + call) % NAMES.len()].1.to_owned()))
                .collect()
        })
        .collect();

    let threads: Vec<_> = (0..8)
        .map(|thread| std::thread::spawn(move || set(thread)))
        .collect();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().map_err(|_| "a thread panicked"))
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(results, expected);

    Ok(())
}

#[test]
fn another_thread_keeps_its_locale() -> Result<(), Box<dyn std::error::Error>> {
    let before = Locale::current();
    let set = thread::spawn(|| darwin_proc::set_locale(c"en_US.UTF-8")).join();

    assert_eq!(
        (
            set.map_err(|_| "the thread panicked")?
                .map(|locale| locale.codeset().to_owned()),
            Locale::current()
        ),
        (Some("UTF-8".to_owned()), before)
    );

    Ok(())
}

// Matching these patterns uses the C library's case conversion and character
// classes, so they match only in the UTF-8 locale. The main thread uses the C
// locale, so a match shows that each expression kept the locale that compiled
// it.
#[test]
fn an_expression_matches_in_the_locale_that_compiled_it() -> Result<(), Box<dyn std::error::Error>>
{
    let compiled = thread::spawn(|| {
        darwin_proc::set_locale(c"en_US.UTF-8").ok_or("en_US.UTF-8 exists")?;

        [("^CAFÉ$", true), ("^caf[[:alpha:]]$", false)]
            .map(|(pattern, ignore_case)| Regex::new(pattern.as_ref(), ignore_case))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.message)
    })
    .join()
    .map_err(|_| "the thread panicked")??;
    let matches: Vec<bool> = compiled
        .iter()
        .map(|regex| regex.is_match("café".as_ref()))
        .collect();

    assert_eq!(
        (Locale::current().codeset().to_owned(), matches),
        ("US-ASCII".to_owned(), vec![true, true])
    );

    Ok(())
}

/// Set in a child's environment, so that the ignored test `child` reports the
/// codeset that the environment selects.
const CHILD: &str = "PROCPS_DARWIN_LOCALE_CHILD";

/// The start of the line in which a child reports the codeset.
const REPORT: &str = "codeset:";

/// Reports the codeset that `set_locale(c"")` selects from this process's
/// environment. The environment tests run this test binary as a child with a
/// chosen environment, because a test cannot change its own environment
/// without `unsafe`.
#[test]
#[ignore = "the environment tests run it as a child"]
fn child() -> std::io::Result<()> {
    if std::env::var_os(CHILD).is_none() {
        return Ok(());
    }

    let codeset = darwin_proc::set_locale(c"").map(|locale| locale.codeset().to_owned());
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{REPORT} {codeset:?}")?;
    stdout.flush()
}

// glibc's `setlocale(LC_ALL, "")` also reads the categories that Darwin lacks,
// such as `LC_PAPER`, and skips a variable whose value is empty.
#[rstest]
#[case::category_that_darwin_lacks(&[("LANG", "C.UTF-8"), ("LC_PAPER", "nope")], None)]
#[case::category_that_darwin_lacks_with_a_locale(
    &[("LANG", "C.UTF-8"), ("LC_PAPER", "C.UTF-8")],
    Some("UTF-8")
)]
#[case::empty_variable(&[("LC_ALL", ""), ("LANG", "en_US.UTF-8")], Some("UTF-8"))]
#[case::category_alone(&[("LC_CTYPE", "en_US.UTF-8")], Some("UTF-8"))]
#[case::posix_in_a_category_that_darwin_lacks(
    &[("LANG", "en_US.UTF-8"), ("LC_PAPER", "POSIX")],
    Some("UTF-8")
)]
fn the_environment_selects_the_locale_as_glibc_selects_it(
    #[case] environment: &[(&str, &str)],
    #[case] expected: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(std::env::current_exe()?)
        .args(["--ignored", "--exact", "--quiet", "child"])
        .env_clear()
        .env(CHILD, "1")
        .envs(environment.iter().copied())
        .stdin(Stdio::null())
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let report = stdout
        .lines()
        .find_map(|line| line.strip_prefix(REPORT))
        .ok_or_else(|| format!("the child reported no codeset: {stdout:?}"))?;

    assert_eq!(report.trim(), format!("{expected:?}"));

    Ok(())
}
