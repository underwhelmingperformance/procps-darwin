// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    cell::RefCell,
    env,
    ffi::{CStr, CString},
    fs,
    marker::PhantomData,
    sync::{Mutex, PoisonError},
};

use crate::ffi;

/// Serialises the calls to `newlocale`. Darwin's loader for every category
/// except `LC_CTYPE` keeps the data that it loaded last in a static cache
/// without a lock, so calls on two threads at once can free the same data.
static NEWLOCALE: Mutex<()> = Mutex::new(());

thread_local! {
    /// The locale that [`set_locale`] made the calling thread's locale.
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// The directory of Darwin's locales.
const LOCALES: &str = "/usr/share/locale";

/// The environment variables of Darwin's locale categories, in the order of
/// the parts of a composite locale name.
const CATEGORIES: [&str; 6] = [
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_MESSAGES",
];

/// The masks of the categories in [`CATEGORIES`], in the same order.
const CATEGORY_MASKS: [libc::c_int; 6] = [
    libc::LC_COLLATE_MASK,
    libc::LC_CTYPE_MASK,
    libc::LC_MONETARY_MASK,
    libc::LC_NUMERIC_MASK,
    libc::LC_TIME_MASK,
    libc::LC_MESSAGES_MASK,
];

/// The environment variables of glibc's locale categories that Darwin lacks.
/// glibc's `setlocale(LC_ALL, "")` fails when one of them has a locale that
/// does not exist. Darwin's `setlocale` ignores these variables.
const GLIBC_ONLY_CATEGORIES: [&str; 6] = [
    "LC_PAPER",
    "LC_NAME",
    "LC_ADDRESS",
    "LC_TELEPHONE",
    "LC_MEASUREMENT",
    "LC_IDENTIFICATION",
];

/// The aliases in glibc 2.44's `intl/locale.alias`, which glibc's `setlocale`
/// replaces with their locales before it reads a name. glibc compares a name
/// with an alias without regard to case.
const GLIBC_ALIASES: [(&str, &str); 45] = [
    ("bokmal", "nb_NO.ISO-8859-1"),
    ("catalan", "ca_ES.ISO-8859-1"),
    ("croatian", "hr_HR.ISO-8859-2"),
    ("czech", "cs_CZ.ISO-8859-2"),
    ("danish", "da_DK.ISO-8859-1"),
    ("dansk", "da_DK.ISO-8859-1"),
    ("deutsch", "de_DE.ISO-8859-1"),
    ("dutch", "nl_NL.ISO-8859-1"),
    ("eesti", "et_EE.ISO-8859-1"),
    ("estonian", "et_EE.ISO-8859-1"),
    ("finnish", "fi_FI.ISO-8859-1"),
    ("french", "fr_FR.ISO-8859-1"),
    ("galego", "gl_ES.ISO-8859-1"),
    ("galician", "gl_ES.ISO-8859-1"),
    ("german", "de_DE.ISO-8859-1"),
    ("greek", "el_GR.ISO-8859-7"),
    ("hebrew", "he_IL.ISO-8859-8"),
    ("hrvatski", "hr_HR.ISO-8859-2"),
    ("hungarian", "hu_HU.ISO-8859-2"),
    ("icelandic", "is_IS.ISO-8859-1"),
    ("italian", "it_IT.ISO-8859-1"),
    ("japanese", "ja_JP.eucJP"),
    ("japanese.euc", "ja_JP.eucJP"),
    ("ja_JP", "ja_JP.eucJP"),
    ("ja_JP.ujis", "ja_JP.eucJP"),
    ("japanese.sjis", "ja_JP.SJIS"),
    ("korean", "ko_KR.eucKR"),
    ("korean.euc", "ko_KR.eucKR"),
    ("ko_KR", "ko_KR.eucKR"),
    ("lithuanian", "lt_LT.ISO-8859-13"),
    ("no_NO", "nb_NO.ISO-8859-1"),
    ("no_NO.ISO-8859-1", "nb_NO.ISO-8859-1"),
    ("norwegian", "nb_NO.ISO-8859-1"),
    ("nynorsk", "nn_NO.ISO-8859-1"),
    ("polish", "pl_PL.ISO-8859-2"),
    ("portuguese", "pt_PT.ISO-8859-1"),
    ("romanian", "ro_RO.ISO-8859-2"),
    ("russian", "ru_RU.ISO-8859-5"),
    ("slovak", "sk_SK.ISO-8859-2"),
    ("slovene", "sl_SI.ISO-8859-2"),
    ("slovenian", "sl_SI.ISO-8859-2"),
    ("spanish", "es_ES.ISO-8859-1"),
    ("swedish", "sv_SE.ISO-8859-1"),
    ("thai", "th_TH.TIS-620"),
    ("turkish", "tr_TR.ISO-8859-9"),
];

/// For each locale name without a codeset, the codeset that glibc gives it. The
/// table covers the languages and territories that Darwin installs, and the
/// codesets come from the locales that nixpkgs builds from glibc's
/// `localedata/SUPPORTED`. [`GLIBC_ALIASES`] covers `ja_JP`, `ko_KR` and
/// `no_NO`.
const GLIBC_CODESETS: [(&str, &str); 95] = [
    ("af_ZA", "ISO-8859-1"),
    ("am_ET", "UTF-8"),
    ("ar_AE", "ISO-8859-6"),
    ("ar_EG", "ISO-8859-6"),
    ("ar_JO", "ISO-8859-6"),
    ("ar_MA", "ISO-8859-6"),
    ("ar_QA", "ISO-8859-6"),
    ("ar_SA", "ISO-8859-6"),
    ("be_BY", "CP1251"),
    ("be_BY@latin", "UTF-8"),
    ("bg_BG", "CP1251"),
    ("ca_AD", "ISO-8859-15"),
    ("ca_ES", "ISO-8859-1"),
    ("ca_ES@euro", "ISO-8859-15"),
    ("ca_ES@valencia", "UTF-8"),
    ("ca_FR", "ISO-8859-15"),
    ("ca_IT", "ISO-8859-15"),
    ("cs_CZ", "ISO-8859-2"),
    ("da_DK", "ISO-8859-1"),
    ("de_AT", "ISO-8859-1"),
    ("de_AT@euro", "ISO-8859-15"),
    ("de_CH", "ISO-8859-1"),
    ("de_DE", "ISO-8859-1"),
    ("de_DE@euro", "ISO-8859-15"),
    ("el_GR", "ISO-8859-7"),
    ("el_GR@euro", "ISO-8859-7"),
    ("en_AU", "ISO-8859-1"),
    ("en_CA", "ISO-8859-1"),
    ("en_GB", "ISO-8859-1"),
    ("en_HK", "ISO-8859-1"),
    ("en_IE", "ISO-8859-1"),
    ("en_IE@euro", "ISO-8859-15"),
    ("en_IN", "UTF-8"),
    ("en_NZ", "ISO-8859-1"),
    ("en_PH", "ISO-8859-1"),
    ("en_SG", "ISO-8859-1"),
    ("en_US", "ISO-8859-1"),
    ("en_ZA", "ISO-8859-1"),
    ("es_AR", "ISO-8859-1"),
    ("es_CR", "ISO-8859-1"),
    ("es_ES", "ISO-8859-1"),
    ("es_ES@euro", "ISO-8859-15"),
    ("es_MX", "ISO-8859-1"),
    ("et_EE", "ISO-8859-1"),
    ("eu_ES", "ISO-8859-1"),
    ("eu_ES@euro", "ISO-8859-15"),
    ("fa_IR", "UTF-8"),
    ("fi_FI", "ISO-8859-1"),
    ("fi_FI@euro", "ISO-8859-15"),
    ("fr_BE", "ISO-8859-1"),
    ("fr_BE@euro", "ISO-8859-15"),
    ("fr_CA", "ISO-8859-1"),
    ("fr_CH", "ISO-8859-1"),
    ("fr_FR", "ISO-8859-1"),
    ("fr_FR@euro", "ISO-8859-15"),
    ("ga_IE", "ISO-8859-1"),
    ("ga_IE@euro", "ISO-8859-15"),
    ("he_IL", "ISO-8859-8"),
    ("hi_IN", "UTF-8"),
    ("hr_HR", "ISO-8859-2"),
    ("hu_HU", "ISO-8859-2"),
    ("hy_AM", "UTF-8"),
    ("is_IS", "ISO-8859-1"),
    ("it_CH", "ISO-8859-1"),
    ("it_IT", "ISO-8859-1"),
    ("it_IT@euro", "ISO-8859-15"),
    ("kk_KZ", "PT154"),
    ("lt_LT", "ISO-8859-13"),
    ("lv_LV", "ISO-8859-13"),
    ("mn_MN", "UTF-8"),
    ("nb_NO", "ISO-8859-1"),
    ("nl_BE", "ISO-8859-1"),
    ("nl_BE@euro", "ISO-8859-15"),
    ("nl_NL", "ISO-8859-1"),
    ("nl_NL@euro", "ISO-8859-15"),
    ("nn_NO", "ISO-8859-1"),
    ("pl_PL", "ISO-8859-2"),
    ("pt_BR", "ISO-8859-1"),
    ("pt_PT", "ISO-8859-1"),
    ("pt_PT@euro", "ISO-8859-15"),
    ("ro_RO", "ISO-8859-2"),
    ("ru_RU", "ISO-8859-5"),
    ("se_NO", "UTF-8"),
    ("sk_SK", "ISO-8859-2"),
    ("sl_SI", "ISO-8859-2"),
    ("sr_RS", "UTF-8"),
    ("sr_RS@latin", "UTF-8"),
    ("sv_FI", "ISO-8859-1"),
    ("sv_FI@euro", "ISO-8859-15"),
    ("sv_SE", "ISO-8859-1"),
    ("tr_TR", "ISO-8859-9"),
    ("uk_UA", "KOI8-U"),
    ("zh_CN", "GB2312"),
    ("zh_HK", "BIG5-HKSCS"),
    ("zh_TW", "BIG5"),
];

/// glibc's locales that have a codeset, with the codeset normalised as glibc
/// normalises it, for the languages that Darwin installs. They are the locales
/// that nixpkgs builds from glibc's `localedata/SUPPORTED`. A name in another
/// language fails on Darwin too, so the table leaves those languages out.
const GLIBC_LOCALES: [&str; 174] = [
    "C.utf8",
    "af_ZA.iso88591",
    "af_ZA.utf8",
    "am_ET.utf8",
    "ar_AE.iso88596",
    "ar_AE.utf8",
    "ar_EG.iso88596",
    "ar_EG.utf8",
    "ar_JO.iso88596",
    "ar_JO.utf8",
    "ar_MA.iso88596",
    "ar_MA.utf8",
    "ar_QA.iso88596",
    "ar_QA.utf8",
    "ar_SA.iso88596",
    "ar_SA.utf8",
    "be_BY.cp1251",
    "be_BY.utf8",
    "be_BY.utf8@latin",
    "bg_BG.cp1251",
    "bg_BG.utf8",
    "ca_AD.iso885915",
    "ca_AD.utf8",
    "ca_ES.iso88591",
    "ca_ES.iso885915@euro",
    "ca_ES.utf8",
    "ca_ES.utf8@valencia",
    "ca_FR.iso885915",
    "ca_FR.utf8",
    "ca_IT.iso885915",
    "ca_IT.utf8",
    "cs_CZ.iso88592",
    "cs_CZ.utf8",
    "da_DK.iso88591",
    "da_DK.utf8",
    "de_AT.iso88591",
    "de_AT.iso885915@euro",
    "de_AT.utf8",
    "de_CH.iso88591",
    "de_CH.utf8",
    "de_DE.iso88591",
    "de_DE.iso885915@euro",
    "de_DE.utf8",
    "el_GR.iso88597",
    "el_GR.iso88597@euro",
    "el_GR.utf8",
    "en_AU.iso88591",
    "en_AU.utf8",
    "en_CA.iso88591",
    "en_CA.utf8",
    "en_GB.iso88591",
    "en_GB.utf8",
    "en_HK.iso88591",
    "en_HK.utf8",
    "en_IE.iso88591",
    "en_IE.iso885915@euro",
    "en_IE.utf8",
    "en_IN.utf8",
    "en_NZ.iso88591",
    "en_NZ.utf8",
    "en_PH.iso88591",
    "en_PH.utf8",
    "en_SG.iso88591",
    "en_SG.utf8",
    "en_US.iso88591",
    "en_US.utf8",
    "en_ZA.iso88591",
    "en_ZA.utf8",
    "es_AR.iso88591",
    "es_AR.utf8",
    "es_CR.iso88591",
    "es_CR.utf8",
    "es_ES.iso88591",
    "es_ES.iso885915@euro",
    "es_ES.utf8",
    "es_MX.iso88591",
    "es_MX.utf8",
    "et_EE.iso88591",
    "et_EE.iso885915",
    "et_EE.utf8",
    "eu_ES.iso88591",
    "eu_ES.iso885915@euro",
    "eu_ES.utf8",
    "fa_IR.utf8",
    "fi_FI.iso88591",
    "fi_FI.iso885915@euro",
    "fi_FI.utf8",
    "fr_BE.iso88591",
    "fr_BE.iso885915@euro",
    "fr_BE.utf8",
    "fr_CA.iso88591",
    "fr_CA.utf8",
    "fr_CH.iso88591",
    "fr_CH.utf8",
    "fr_FR.iso88591",
    "fr_FR.iso885915@euro",
    "fr_FR.utf8",
    "ga_IE.iso88591",
    "ga_IE.iso885915@euro",
    "ga_IE.utf8",
    "he_IL.iso88598",
    "he_IL.utf8",
    "hi_IN.utf8",
    "hr_HR.iso88592",
    "hr_HR.utf8",
    "hu_HU.iso88592",
    "hu_HU.utf8",
    "hy_AM.armscii8",
    "hy_AM.utf8",
    "is_IS.iso88591",
    "is_IS.utf8",
    "it_CH.iso88591",
    "it_CH.utf8",
    "it_IT.iso88591",
    "it_IT.iso885915@euro",
    "it_IT.utf8",
    "ja_JP.eucjp",
    "ja_JP.utf8",
    "kk_KZ.pt154",
    "kk_KZ.utf8",
    "ko_KR.euckr",
    "ko_KR.utf8",
    "lt_LT.iso885913",
    "lt_LT.utf8",
    "lv_LV.iso885913",
    "lv_LV.utf8",
    "mn_MN.utf8",
    "nb_NO.iso88591",
    "nb_NO.utf8",
    "nl_BE.iso88591",
    "nl_BE.iso885915@euro",
    "nl_BE.utf8",
    "nl_NL.iso88591",
    "nl_NL.iso885915@euro",
    "nl_NL.utf8",
    "nn_NO.iso88591",
    "nn_NO.utf8",
    "pl_PL.iso88592",
    "pl_PL.utf8",
    "pt_BR.iso88591",
    "pt_BR.utf8",
    "pt_PT.iso88591",
    "pt_PT.iso885915@euro",
    "pt_PT.utf8",
    "ro_RO.iso88592",
    "ro_RO.utf8",
    "ru_RU.iso88595",
    "ru_RU.koi8r",
    "ru_RU.utf8",
    "se_NO.utf8",
    "sk_SK.iso88592",
    "sk_SK.utf8",
    "sl_SI.iso88592",
    "sl_SI.utf8",
    "sr_RS.utf8",
    "sr_RS.utf8@latin",
    "sv_FI.iso88591",
    "sv_FI.iso885915@euro",
    "sv_FI.utf8",
    "sv_SE.iso88591",
    "sv_SE.utf8",
    "tr_TR.iso88599",
    "tr_TR.utf8",
    "uk_UA.koi8u",
    "uk_UA.utf8",
    "zh_CN.gb18030",
    "zh_CN.gb2312",
    "zh_CN.gbk",
    "zh_CN.utf8",
    "zh_HK.big5hkscs",
    "zh_HK.utf8",
    "zh_TW.big5",
    "zh_TW.euctw",
    "zh_TW.utf8",
];

/// The locale names that Darwin installs without a codeset, and that glibc
/// lacks.
const GLIBC_LACKS: [&str; 3] = ["fa_AF", "se_FI", "sr_YU"];

/// Sets the calling thread's locale to `name`, as `setlocale(LC_ALL, name)`
/// sets the process's locale. procps-ng calls it with `""` at start-up.
/// procps-ng's tools have one thread, so the result is the same for them.
/// Other threads keep their locales, so a call cannot race with a thread that
/// uses the C library.
///
/// The empty name selects the locale of each category from the environment:
/// `LC_ALL`, the category's own variable, or `LANG`. The call fails as a whole
/// if the environment sets any category to a locale that does not exist, and
/// the locale then stays as it was. The tools use `LC_CTYPE`, which decides the
/// character encoding, how characters are classified and how their case
/// folds. A [`Regex`](crate::Regex) also reads the name of the `LC_COLLATE`
/// locale, to tell whether glibc would have collation rules for a locale of
/// that name.
///
/// This function reads locale names as glibc reads them:
///
/// - glibc normalises the codeset in a locale name, so it accepts `C.utf8` and
///   `en_US.ISO-8859-1`. Darwin accepts only the spellings of its installed
///   locales, such as `C.UTF-8` and `en_US.ISO8859-1`. This function uses the
///   installed locale whose codeset is the same after glibc's normalisation.
/// - A name without a codeset, such as `en_US`, has glibc's codeset for that
///   name, here ISO 8859-1. Darwin's `en_US` is a UTF-8 locale. If Darwin has
///   no locale with glibc's codeset, this function uses Darwin's locale, and it
///   fails for a name that glibc lacks.
/// - A name with a codeset fails unless it is among the locales that nixpkgs
///   generates from glibc's `localedata/SUPPORTED`, which has no
///   `en_US.ISO-8859-15`, for example. A Linux system can generate other
///   locales, which this function does not accept.
/// - An alias from glibc's `locale.alias`, such as `german` or
///   `no_NO.ISO-8859-1`, is replaced with its locale. The comparison ignores
///   case.
/// - A modifier that Darwin lacks, such as `@euro`, is left out: `de_DE@euro`
///   is `de_DE.ISO8859-15`.
/// - For the empty name, the categories that only glibc has, such as
///   `LC_PAPER`, must have locales that exist too.
///
/// A name with slashes is a composite name in Darwin's syntax, with the locales
/// of `LC_COLLATE`, `LC_CTYPE`, `LC_MONETARY`, `LC_NUMERIC`, `LC_TIME` and
/// `LC_MESSAGES` in that order.
///
/// Returns the new locale, or `None` if the call fails. Call this before
/// compiling a [`Regex`](crate::Regex), because `regcomp` reads the pattern in
/// the calling thread's locale.
///
/// ```
/// let locale = darwin_proc::set_locale(c"C").expect("the C locale exists");
///
/// assert_eq!(locale.codeset(), "US-ASCII");
/// ```
#[must_use]
pub fn set_locale(name: &CStr) -> Option<Locale> {
    let locale = OwnedLocale::new(&darwin_name(name)?)?;
    let installed = Installed::new(locale);

    drop(INSTALLED.with(|current| current.replace(Some(installed))));

    Some(Locale::current())
}

/// The parts of a thread's locale that the tools read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Locale {
    codeset: String,
    case_folding: CaseFolding,
}

impl Locale {
    /// The calling thread's locale.
    ///
    /// ```
    /// assert!(!darwin_proc::Locale::current().codeset().is_empty());
    /// ```
    #[must_use]
    pub fn current() -> Self {
        // SAFETY: `nl_langinfo` returns a pointer to a NUL-terminated string
        // that remains valid until the calling thread's locale changes, and the
        // string is copied first.
        let codeset = unsafe { CStr::from_ptr(libc::nl_langinfo(libc::CODESET)) }
            .to_string_lossy()
            .into_owned();

        // SAFETY: `uselocale` with a null pointer returns the calling thread's
        // locale without changing it. `querylocale` returns the name of that
        // locale's `LC_CTYPE` locale, or a null pointer. The name remains valid
        // while the locale is in use, and it is copied first.
        let ctype = unsafe {
            let name =
                libc::querylocale(libc::LC_CTYPE_MASK, libc::uselocale(std::ptr::null_mut()));

            (!name.is_null()).then(|| CStr::from_ptr(name).to_string_lossy().into_owned())
        };

        Self {
            case_folding: CaseFolding::for_locale(ctype.as_deref().unwrap_or("C"), &codeset),
            codeset,
        }
    }

    /// The locale's character encoding, as `nl_langinfo(CODESET)` reports it,
    /// such as `UTF-8`.
    ///
    /// ```
    /// let locale = darwin_proc::set_locale(c"en_US.UTF-8").expect("en_US.UTF-8 exists");
    ///
    /// assert_eq!(locale.codeset(), "UTF-8");
    /// ```
    #[must_use]
    pub fn codeset(&self) -> &str {
        &self.codeset
    }

    /// How glibc folds case in the locale's `LC_CTYPE` locale.
    ///
    /// ```
    /// use darwin_proc::CaseFolding;
    ///
    /// let locale = darwin_proc::set_locale(c"tr_TR.UTF-8").expect("tr_TR.UTF-8 exists");
    ///
    /// assert_eq!(locale.case_folding(), CaseFolding::TurkishUtf8);
    /// ```
    #[must_use]
    pub const fn case_folding(&self) -> CaseFolding {
        self.case_folding
    }
}

/// How glibc folds the case of a byte that could match an ASCII letter, in an
/// `LC_CTYPE` locale. glibc's `strcasecmp` and `toupper` follow these rules,
/// and procps-ng compares signal names with them.
///
/// glibc's Turkish locales pair `i` with `İ` and `I` with `ı`. In UTF-8, those
/// letters are not single bytes, so `toupper('i')` and `tolower('I')` return
/// the byte unchanged, and `i` and `I` do not match each other. In ISO
/// 8859-9, they are the bytes 0xDD and 0xFD. glibc gives `az_AZ`, `crh_UA`,
/// `ku_TR`, `tr_CY` and `tt_RU@iqtelif` the same rules, and `mt_MT` rules of
/// its own, but Darwin installs none of them, so the tools cannot select them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CaseFolding {
    /// Each ASCII letter folds to the same letter in the other case, and no
    /// other byte folds to or from an ASCII letter.
    #[default]
    Ascii,
    /// glibc's Turkish UTF-8 locales: as [`CaseFolding::Ascii`], except that
    /// `i` and `I` have no other case.
    TurkishUtf8,
    /// glibc's Turkish ISO 8859-9 locales: as [`CaseFolding::Ascii`], except
    /// that `I` folds to `ı` (0xFD) and `i` to `İ` (0xDD).
    TurkishLatin5,
}

impl CaseFolding {
    /// The folding of the `LC_CTYPE` locale `name`, whose codeset is
    /// `codeset`.
    fn for_locale(name: &str, codeset: &str) -> Self {
        if LocaleName::parse(name).language != "tr_TR" {
            return Self::Ascii;
        }

        match normalise_codeset(codeset).as_str() {
            "utf8" => Self::TurkishUtf8,
            "iso88599" => Self::TurkishLatin5,
            _ => Self::Ascii,
        }
    }

    /// glibc's `tolower` of `byte` for ASCII letters and for the bytes that
    /// fold to or from them. Other bytes are returned unchanged. glibc would
    /// change some of them, but none becomes an ASCII letter or 0xFD, so
    /// comparisons with ASCII text have the same results.
    const fn lower(self, byte: u8) -> u8 {
        match (self, byte) {
            (Self::TurkishUtf8, b'I') => b'I',
            (Self::TurkishLatin5, b'I') => 0xfd,
            (Self::TurkishLatin5, 0xdd) => b'i',
            _ => byte.to_ascii_lowercase(),
        }
    }

    /// Whether `text` is `ascii` without regard to case, as glibc's
    /// `strcasecmp` compares them. `ascii` must be ASCII.
    ///
    /// ```
    /// use darwin_proc::CaseFolding;
    ///
    /// assert!(CaseFolding::Ascii.eq_ignore_case(b"kill", b"KILL"));
    /// assert!(!CaseFolding::TurkishUtf8.eq_ignore_case(b"kill", b"KILL"));
    /// ```
    #[must_use]
    pub fn eq_ignore_case(self, text: &[u8], ascii: &[u8]) -> bool {
        text.len() == ascii.len() && self.starts_with_ignore_case(text, ascii)
    }

    /// Whether `text` starts with `ascii` without regard to case, as glibc's
    /// `strncasecmp(text, ascii, n)` compares them, where `n` is the length of
    /// `ascii`. `ascii` must be ASCII.
    ///
    /// ```
    /// use darwin_proc::CaseFolding;
    ///
    /// assert!(CaseFolding::TurkishLatin5.starts_with_ignore_case(b"S\xfdGINT", b"SIG"));
    /// ```
    #[must_use]
    pub fn starts_with_ignore_case(self, text: &[u8], ascii: &[u8]) -> bool {
        debug_assert!(ascii.is_ascii(), "the comparison is exact only for ASCII");

        text.len() >= ascii.len()
            && text
                .iter()
                .zip(ascii)
                .all(|(&byte, &other)| self.lower(byte) == self.lower(other))
    }
}

/// A locale object that this crate created with `newlocale` or `duplocale`.
/// Dropping it frees the object.
pub(crate) struct OwnedLocale(libc::locale_t);

// SAFETY: Darwin counts a locale object's references atomically. It changes an
// object after creating it only in the conversion states that functions such as
// `mbrtowc` use when they are given no state, and in the `LC_NUMERIC` cache of
// the `strtod` family. This crate calls neither kind of function with these
// objects, and glibc's regular expression code passes its own state to every
// conversion, so several threads can use one object at the same time.
unsafe impl Send for OwnedLocale {}

// SAFETY: as for `Send`.
unsafe impl Sync for OwnedLocale {}

impl OwnedLocale {
    /// The locale with the names in `parts`: a single name for all categories,
    /// or one name for each category in [`CATEGORIES`]. Darwin's `newlocale`
    /// does not read composite names, so each category is set on its own.
    fn new(parts: &[String]) -> Option<Self> {
        let names: Vec<CString> = parts
            .iter()
            .map(|part| CString::new(part.as_str()).ok())
            .collect::<Option<_>>()?;

        if let [name] = names.as_slice() {
            return Self::create(libc::LC_ALL_MASK, name, std::ptr::null_mut());
        }

        if names.len() != CATEGORY_MASKS.len() {
            return None;
        }

        let c = Self::create(libc::LC_ALL_MASK, c"C", std::ptr::null_mut())?;

        CATEGORY_MASKS
            .into_iter()
            .zip(&names)
            .try_fold(c, |locale, (mask, name)| Self::create(mask, name, locale.0))
    }

    /// A copy of `base` with the categories in `mask` set to the locale `name`,
    /// or `None` if Darwin has no locale `name`. Darwin's `newlocale` copies
    /// `base` and leaves it unchanged, unlike the `newlocale` that POSIX
    /// describes, so the caller still owns `base`. For `LC_ALL_MASK`, Darwin
    /// ignores `base`.
    fn create(mask: libc::c_int, name: &CStr, base: libc::locale_t) -> Option<Self> {
        let _newlocale = NEWLOCALE.lock().unwrap_or_else(PoisonError::into_inner);

        // SAFETY: `name` is a NUL-terminated string that outlives the call,
        // and `base` is null or a locale object that the caller owns, which
        // the call only reads. The lock keeps other calls to `newlocale` out.
        let locale = unsafe { libc::newlocale(mask, name.as_ptr(), base) };

        (!locale.is_null()).then_some(Self(locale))
    }

    /// A copy of the calling thread's locale, or `None` if there is not enough
    /// memory.
    pub(crate) fn current() -> Option<Self> {
        // SAFETY: `uselocale` with a null pointer returns the calling thread's
        // locale without changing it, and `duplocale` copies it.
        let locale = unsafe { libc::duplocale(libc::uselocale(std::ptr::null_mut())) };

        (!locale.is_null()).then_some(Self(locale))
    }

    /// Makes this locale the calling thread's locale until the guard is
    /// dropped.
    pub(crate) fn enter(&self) -> Entered<'_> {
        // SAFETY: `self.0` is a locale object, which outlives the guard.
        // `uselocale` returns the thread's previous locale, which the guard
        // restores.
        let previous = unsafe { libc::uselocale(self.0) };

        Entered {
            previous,
            locale: PhantomData,
        }
    }
}

impl Drop for OwnedLocale {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }

        // SAFETY: `self.0` is a locale object that this crate owns, and no
        // thread uses it: `Installed` and `Entered` stop using it before
        // it is dropped.
        unsafe { libc::freelocale(self.0) };
    }
}

/// The calling thread uses an [`OwnedLocale`] until this guard is dropped,
/// which restores the thread's previous locale.
pub(crate) struct Entered<'a> {
    previous: libc::locale_t,
    locale: PhantomData<&'a OwnedLocale>,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        // SAFETY: `previous` is the locale that the thread used before, which
        // is `LC_GLOBAL_LOCALE` or a locale object that is still alive.
        unsafe { libc::uselocale(self.previous) };
    }
}

/// The locale that [`set_locale`] made the calling thread's locale. When the
/// thread exits, the thread returns to the global locale before the locale is
/// freed.
struct Installed(OwnedLocale);

impl Installed {
    fn new(locale: OwnedLocale) -> Self {
        // SAFETY: `locale.0` is a locale object, which this value keeps alive
        // while the thread uses it.
        unsafe { libc::uselocale(locale.0) };

        Self(locale)
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        // SAFETY: `uselocale` with a null pointer only returns the thread's
        // locale, and `LC_GLOBAL_LOCALE` makes the thread use the global
        // locale again.
        unsafe {
            if libc::uselocale(std::ptr::null_mut()) == (self.0).0 {
                libc::uselocale(ffi::LC_GLOBAL_LOCALE);
            }
        }
    }
}

/// Rewrites each part of a locale name as [`darwin_locale`] rewrites it. The
/// empty name becomes the locale of each category in the environment.
///
/// Returns `None` if a part is a locale that glibc lacks, or if the name is not
/// UTF-8. For the empty name, it also returns `None` if a category that only
/// glibc has refers to a locale that Darwin does not install, or if a value in
/// the environment is not UTF-8 or contains a slash. A slash would split the
/// value into two parts of the composite name.
fn darwin_name(name: &CStr) -> Option<Vec<String>> {
    let parts: Vec<String> = if name.is_empty() {
        for category in GLIBC_ONLY_CATEGORIES {
            if !is_installed(&darwin_locale(&environment_locale(category)?)?) {
                return None;
            }
        }

        CATEGORIES
            .iter()
            .map(|category| environment_locale(category))
            .collect::<Option<_>>()?
    } else {
        name.to_str().ok()?.split('/').map(str::to_owned).collect()
    };

    parts.iter().map(|part| darwin_locale(part)).collect()
}

/// The installed Darwin locale for a glibc locale name, in Darwin's spelling.
/// An alias from [`GLIBC_ALIASES`] is first replaced with its locale. A name
/// without a codeset gets glibc's codeset for that name from
/// [`GLIBC_CODESETS`]. Returns `None` for a name with a codeset that is not in
/// [`GLIBC_LOCALES`], and for a name in [`GLIBC_LACKS`]. [`GLIBC_LOCALES`]
/// lists the locales that nixpkgs generates from glibc's
/// `localedata/SUPPORTED`, so the call fails as glibc's fails on a system that
/// has generated only those locales. Otherwise, if Darwin does not install the
/// locale, the result is not an installed locale either, and Darwin fails to
/// set it.
///
/// glibc has modifiers that Darwin lacks, such as the `@euro` of `de_DE@euro`.
/// When glibc has a codeset for the name, and Darwin has no locale with the
/// modifier, the modifier is left out, so `de_DE@euro` is `de_DE.ISO8859-15`.
/// Darwin's only modifier is the `@latin` of `sr_RS.UTF-8@latin`, which glibc
/// also has.
fn darwin_locale(name: &str) -> Option<String> {
    let name = GLIBC_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map_or(name, |&(_, locale)| locale);
    let locale = LocaleName::parse(name);

    if let Some(codeset) = locale.codeset {
        let glibc_name = locale.modifier.map_or_else(
            || format!("{}.{}", locale.language, normalise_codeset(codeset)),
            |modifier| {
                format!(
                    "{}.{}@{modifier}",
                    locale.language,
                    normalise_codeset(codeset)
                )
            },
        );

        if !GLIBC_LOCALES.contains(&glibc_name.as_str()) {
            return None;
        }

        let without_modifier = LocaleName {
            modifier: None,
            ..locale
        };

        return Some(
            locale
                .darwin_spelling()
                .or_else(|| without_modifier.darwin_spelling())
                .unwrap_or_else(|| name.to_owned()),
        );
    }

    if locale.modifier.is_none() && GLIBC_LACKS.contains(&locale.language) {
        return None;
    }

    let key = locale.modifier.map_or_else(
        || locale.language.to_owned(),
        |modifier| format!("{}@{modifier}", locale.language),
    );
    let Some(&(_, codeset)) = GLIBC_CODESETS
        .iter()
        .find(|(glibc_name, _)| *glibc_name == key)
    else {
        return Some(name.to_owned());
    };
    let with_codeset = LocaleName {
        codeset: Some(codeset),
        ..locale
    };
    let without_modifier = LocaleName {
        modifier: None,
        ..with_codeset
    };

    Some(
        with_codeset
            .darwin_spelling()
            .or_else(|| without_modifier.darwin_spelling())
            .unwrap_or_else(|| {
                LocaleName {
                    modifier: None,
                    ..locale
                }
                .to_string()
            }),
    )
}

/// Whether Darwin has the locale `name`: `C`, `POSIX` or an installed locale.
fn is_installed(name: &str) -> bool {
    name == "C" || name == "POSIX" || std::path::Path::new(LOCALES).join(name).is_dir()
}

/// The locale that the environment sets for a category, as glibc and Darwin
/// read it: `LC_ALL`, the category's own variable, then `LANG`, ignoring empty
/// values, and `C` if none of them is set. Returns `None` if the value is not
/// UTF-8 or contains a slash.
fn environment_locale(category: &str) -> Option<String> {
    let value = ["LC_ALL", category, "LANG"]
        .into_iter()
        .filter_map(env::var_os)
        .find(|value| !value.is_empty());

    let Some(value) = value else {
        return Some("C".to_owned());
    };

    let value = value.into_string().ok()?;

    if value.contains('/') {
        return None;
    }

    Some(value)
}

/// The parts of a locale name of the form `language.codeset@modifier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocaleName<'a> {
    language: &'a str,
    codeset: Option<&'a str>,
    modifier: Option<&'a str>,
}

impl std::fmt::Display for LocaleName<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.language)?;

        if let Some(codeset) = self.codeset {
            write!(formatter, ".{codeset}")?;
        }

        if let Some(modifier) = self.modifier {
            write!(formatter, "@{modifier}")?;
        }

        Ok(())
    }
}

impl<'a> LocaleName<'a> {
    fn parse(name: &'a str) -> Self {
        let (name, modifier) = name
            .split_once('@')
            .map_or((name, None), |(name, modifier)| (name, Some(modifier)));

        let (language, codeset) = name
            .split_once('.')
            .map_or((name, None), |(language, codeset)| {
                (language, Some(codeset))
            });

        Self {
            language,
            codeset,
            modifier,
        }
    }

    /// The name of the installed Darwin locale with this language and
    /// modifier, and a codeset that is the same after normalisation.
    fn darwin_spelling(self) -> Option<String> {
        let codeset = normalise_codeset(self.codeset?);

        fs::read_dir(LOCALES)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .find(|entry| {
                let installed = LocaleName::parse(entry);

                installed.language == self.language
                    && installed.modifier == self.modifier
                    && installed
                        .codeset
                        .is_some_and(|installed| normalise_codeset(installed) == codeset)
            })
    }
}

/// Normalises a codeset as glibc's `_nl_normalize_codeset` does: it keeps only
/// the letters and digits, in lower case, and adds `iso` before a codeset of
/// digits alone.
fn normalise_codeset(codeset: &str) -> String {
    let normalised: String = codeset
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();

    if normalised
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        return format!("iso{normalised}");
    }

    normalised
}

/// The character that each byte encodes on its own in the character encoding
/// `codeset`, decoded with `iconv`, or `None` for a byte that encodes no
/// character on its own. Returns `None` if `iconv` does not know `codeset`.
///
/// ```
/// let decoded = darwin_proc::decode_each_byte(c"ISO8859-1").expect("iconv knows ISO 8859-1");
///
/// assert_eq!((decoded[0x41], decoded[0xe9]), (Some('A'), Some('é')));
/// ```
#[must_use]
pub fn decode_each_byte(codeset: &CStr) -> Option<[Option<char>; 256]> {
    // SAFETY: both arguments are NUL-terminated strings that outlive the call.
    let descriptor = unsafe { ffi::iconv_open(c"UTF-32LE".as_ptr(), codeset.as_ptr()) };

    if descriptor.addr() == usize::MAX {
        return None;
    }

    let mut decoded = [None; 256];

    for (byte, slot) in (0..=u8::MAX).zip(&mut decoded) {
        let mut input = [byte];
        let mut output = [0_u8; 4];
        let mut input_pointer = input.as_mut_ptr().cast::<libc::c_char>();
        let mut input_left = input.len();
        let mut output_pointer = output.as_mut_ptr().cast::<libc::c_char>();
        let mut output_left = output.len();

        // SAFETY: `descriptor` is open. Null pointers reset its shift state.
        // The other pointers refer to `input` and `output`, and the counts are
        // their lengths.
        let result = unsafe {
            ffi::iconv(
                descriptor,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            ffi::iconv(
                descriptor,
                &raw mut input_pointer,
                &raw mut input_left,
                &raw mut output_pointer,
                &raw mut output_left,
            )
        };

        if result != usize::MAX && input_left == 0 && output_left == 0 {
            *slot = char::from_u32(u32::from_le_bytes(output));
        }
    }

    // SAFETY: `descriptor` is open, and nothing uses it after this call.
    unsafe { ffi::iconv_close(descriptor) };

    Some(decoded)
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use pretty_assertions::assert_eq;
    use rstest::rstest;

    use super::{CaseFolding, LocaleName, darwin_locale, decode_each_byte, normalise_codeset};

    #[rstest]
    #[case::linux("utf8", "utf8")]
    #[case::darwin("UTF-8", "utf8")]
    #[case::iso("ISO-8859-1", "iso88591")]
    #[case::digits("8859_1", "iso88591")]
    #[case::mixed_case("eucJP", "eucjp")]
    fn a_codeset_is_normalised_as_glibc_normalises_it(
        #[case] codeset: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(normalise_codeset(codeset), expected);
    }

    #[rstest]
    #[case::language("en_US", LocaleName { language: "en_US", codeset: None, modifier: None })]
    #[case::codeset("C.utf8", LocaleName { language: "C", codeset: Some("utf8"), modifier: None })]
    #[case::modifier(
        "sr_RS.UTF-8@latin",
        LocaleName { language: "sr_RS", codeset: Some("UTF-8"), modifier: Some("latin") },
    )]
    fn a_locale_name_has_parts(#[case] name: &str, #[case] expected: LocaleName<'_>) {
        assert_eq!(LocaleName::parse(name), expected);
    }

    #[rstest]
    #[case::c("C", Some("C"))]
    #[case::linux("C.utf8", Some("C.UTF-8"))]
    #[case::modifier("sr_RS.utf8@latin", Some("sr_RS.UTF-8@latin"))]
    #[case::modifier_that_glibc_lacks("sr_RS.utf8@cyrillic", None)]
    #[case::unknown_codeset("en_US.nope", None)]
    #[case::codeset_that_glibc_lacks("en_US.ISO8859-15", None)]
    #[case::darwin_codeset_that_glibc_lacks("ja_JP.SJIS", None)]
    #[case::language_that_glibc_lacks("no_NO.UTF-8", None)]
    #[case::locale_that_glibc_lacks_with_a_codeset("sr_YU.UTF-8", None)]
    #[case::modifier_that_darwin_lacks("de_DE.ISO-8859-15@euro", Some("de_DE.ISO8859-15"))]
    #[case::language_that_darwin_lacks("en_DK.UTF-8", None)]
    #[case::no_codeset("en_US", Some("en_US.ISO8859-1"))]
    #[case::no_codeset_in_utf8("en_IN", Some("en_IN.UTF-8"))]
    #[case::codeset_that_darwin_lacks("he_IL", Some("he_IL"))]
    #[case::locale_that_glibc_lacks("sr_YU", None)]
    #[case::modifier_without_codeset("de_DE@euro", Some("de_DE.ISO8859-15"))]
    #[case::modifier_that_darwin_has("sr_RS@latin", Some("sr_RS.UTF-8@latin"))]
    #[case::modifier_and_codeset_that_darwin_lacks("ga_IE@euro", Some("ga_IE"))]
    #[case::alias_with_a_codeset("no_NO.ISO-8859-1", Some("nb_NO.ISO8859-1"))]
    #[case::alias_in_another_case("NO_no.iso-8859-1", Some("nb_NO.ISO8859-1"))]
    #[case::alias_without_a_codeset("no_NO", Some("nb_NO.ISO8859-1"))]
    #[case::word_alias("german", Some("de_DE.ISO8859-1"))]
    #[case::alias_for_a_locale_that_glibc_lacks("japanese.sjis", None)]
    #[case::darwin_spelling_of_an_alias("no_NO.ISO8859-1", None)]
    fn a_glibc_locale_has_a_darwin_locale(#[case] name: &str, #[case] expected: Option<&str>) {
        assert_eq!(darwin_locale(name).as_deref(), expected);
    }

    #[rstest]
    #[case::letter(c"ISO8859-1", 0xe9, Some('é'))]
    #[case::control(c"ISO8859-1", 0x85, Some('\u{85}'))]
    #[case::unassigned(c"ISO8859-3", 0xa5, None)]
    #[case::cyrillic(c"KOI8-R", 0xc1, Some('а'))]
    #[case::ascii(c"US-ASCII", 0xe9, None)]
    fn each_byte_is_decoded_on_its_own(
        #[case] codeset: &CStr,
        #[case] byte: u8,
        #[case] expected: Option<char>,
    ) {
        let decoded = decode_each_byte(codeset).expect("iconv knows the codeset");

        assert_eq!(decoded[usize::from(byte)], expected);
    }

    #[test]
    fn an_unknown_codeset_is_not_decoded() {
        assert_eq!(decode_each_byte(c"NO-SUCH-CODESET"), None);
    }

    // glibc 2.44's `strcasecmp` gives these results in `tr_TR.ISO-8859-9`.
    #[rstest]
    #[case::dotted_capital_i(b"K\xddLL", b"kill", true)]
    #[case::dotless_small_i(b"k\xfdll", b"KILL", true)]
    #[case::small_and_capital_i(b"kill", b"KILL", false)]
    fn latin5_folds_the_turkish_i_as_glibc_folds_it(
        #[case] text: &[u8],
        #[case] ascii: &[u8],
        #[case] expected: bool,
    ) {
        assert_eq!(
            CaseFolding::TurkishLatin5.eq_ignore_case(text, ascii),
            expected
        );
    }
}
