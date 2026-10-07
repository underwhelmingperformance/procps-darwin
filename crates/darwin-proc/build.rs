// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles glibc's regular expression code in `glibc-regex`, for
//! `src/regex.rs` to call. It also compiles `c/case.c` and `c/collation.c`,
//! which the regular expression code calls to fold case and to check the
//! locale, and the ABI check in `c/regex_abi.c`.

fn main() {
    println!("cargo::rerun-if-changed=c");
    println!("cargo::rerun-if-changed=glibc-regex");
    // The variable decides whether the Nix compiler wrapper warns about the
    // target. Cargo replays a build script's warnings until the script runs
    // again, so a change of the variable must run it again.
    println!("cargo::rerun-if-env-changed=NIX_CC_WRAPPER_SUPPRESS_TARGET_WARNING");

    // glibc builds this code with `-Wall` but not `-Wextra`. The code
    // triggers `-Wextra`'s warnings about signed comparisons and unused
    // parameters.
    //
    // Without `_USE_EXTENDED_LOCALES_`, Darwin's `<stdlib.h>` defines
    // `MB_CUR_MAX` as the value for the global locale, and `set_locale` sets
    // only the calling thread's locale, so the regular expression code would
    // read the global locale's value.
    cc::Build::new()
        .extra_warnings(false)
        .define("_USE_EXTENDED_LOCALES_", None)
        .include("glibc-regex")
        .include("c")
        .file("glibc-regex/regex.c")
        .file("c/case.c")
        .file("c/collation.c")
        .file("glibc-regex/malloc/dynarray_resize.c")
        .file("c/regex_abi.c")
        .compile("glibc-regex");
}
