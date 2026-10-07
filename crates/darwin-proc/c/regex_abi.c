/*
 * SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/* src/regex.rs cannot include regex.h, so it declares glibc's flags and
   error codes itself.  It also declares storage with the size and alignment
   of regex_t.
   This file exports the values from regex.h, and a test in src/regex.rs
   compares them with those declarations.  */

#include <config.h>
#include <regex.h>
#include <stddef.h>

struct procps_regex_abi
{
  size_t size;
  size_t alignment;
  int flags[3];
  int errors[14];
};

const struct procps_regex_abi procps_regex_abi = {
  .size = sizeof (regex_t),
  .alignment = _Alignof (regex_t),
  .flags = { REG_EXTENDED, REG_ICASE, REG_NOSUB },
  .errors = {
    REG_BADPAT, REG_ECOLLATE, REG_ECTYPE, REG_EESCAPE, REG_ESUBREG,
    REG_EBRACK, REG_EPAREN, REG_EBRACE, REG_BADBR, REG_ERANGE,
    REG_ESPACE, REG_BADRPT, REG_EEND, REG_ESIZE,
  },
};
