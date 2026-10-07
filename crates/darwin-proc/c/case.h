/*
 * SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef PROCPS_DARWIN_CASE_H
#define PROCPS_DARWIN_CASE_H

#include <wctype.h>

/* How the regular expression code folds case on the calling thread.  The
   values are those of darwin_proc::CaseFolding.  */
enum procps_case_folding
{
  PROCPS_CASE_ASCII = 0,
  PROCPS_CASE_TURKISH_UTF8 = 1,
  PROCPS_CASE_TURKISH_LATIN5 = 2,
};

/* Makes FOLDING the calling thread's case folding, and returns the previous
   one.  */
int procps_set_case_folding (int folding);

/* These behave as toupper, tolower, towupper and towlower in the current
   locale.  In a Turkish locale, they fold i, I, the dotted capital I and the
   dotless small i as glibc's Turkish locales do.  Darwin's Turkish locales
   use the generic case tables, which pair i with I.  */
int procps_toupper (int c);
int procps_tolower (int c);
wint_t procps_towupper (wint_t wc);
wint_t procps_towlower (wint_t wc);

#endif
