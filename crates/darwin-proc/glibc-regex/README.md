<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# glibc's regular expressions

`darwin_proc::Regex` compiles and matches patterns with the code in this
directory, so that `pgrep` and `pkill` read patterns as procps-ng does on Linux.

- `regex.c`, `regcomp.c`, `regexec.c`, `regex_internal.c`, `regex_internal.h`
  and `regex.h` are from glibc 2.44's `posix/` directory. `regcomp.c` and
  `regex_internal.h` have these changes, each marked `procps-darwin`:
  - `regcomp.c` includes `collation.h` from `../c`, which `build.rs` adds to the
    include path.
  - When the code is built without `_LIBC`, as here, and the `LC_COLLATE` locale
    has no collation rules, `build_range_exp` in `regcomp.c` compares the ends
    of a range as glibc does: it compares a single byte by its own value, and it
    rejects a range with a multi-byte end. `../c/collation.c` decides whether
    the locale has rules.
  - `regex_internal.h` folds case with the functions in `../c/case.c`. In a
    Turkish locale, they pair `i` with the dotted capital I and `I` with the
    dotless small i, as glibc's Turkish locales do. Darwin's Turkish locales
    pair `i` with `I`.
  - `regex_internal.h` stops the build if `regexec` would not lock the compiled
    expression. `darwin_proc::Regex` lets threads share a compiled expression.
- The other files, which glibc's code needs outside glibc, are from gnulib
  commit `d89ea630c1318a1a595aa3aff169586f4e626274`. `config.h` is the output of
  gnulib's `configure` on macOS 27 on arm64, after
  `gnulib-tool --create-testdir --without-tests --single-configure regex`.

`config.h` renames the functions to `rpl_regcomp`, `rpl_regexec`, `rpl_regerror`
and `rpl_regfree`, so they do not clash with Darwin's.

Use glibc's files, not gnulib's copies of them. gnulib's copies have changed
since glibc 2.44: for example, with `REG_ICASE` in a single-byte locale, a
bracket expression such as `[a]` no longer matches `a`.
