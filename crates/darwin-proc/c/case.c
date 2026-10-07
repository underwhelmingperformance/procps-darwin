/*
 * SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include <ctype.h>
#include <wctype.h>

#include "case.h"

/* In ISO 8859-9, the dotted capital I and the dotless small i.  */
#define LATIN5_CAPITAL_I_WITH_DOT 0xdd
#define LATIN5_SMALL_DOTLESS_I 0xfd

static _Thread_local int folding = PROCPS_CASE_ASCII;

int
procps_set_case_folding (int new_folding)
{
  int previous = folding;

  folding = new_folding;
  return previous;
}

int
procps_toupper (int c)
{
  if (folding == PROCPS_CASE_TURKISH_UTF8 && c == 'i')
    return c;
  if (folding == PROCPS_CASE_TURKISH_LATIN5)
    {
      if (c == 'i')
	return LATIN5_CAPITAL_I_WITH_DOT;
      if (c == LATIN5_SMALL_DOTLESS_I)
	return 'I';
    }
  return toupper (c);
}

int
procps_tolower (int c)
{
  if (folding == PROCPS_CASE_TURKISH_UTF8 && c == 'I')
    return c;
  if (folding == PROCPS_CASE_TURKISH_LATIN5)
    {
      if (c == 'I')
	return LATIN5_SMALL_DOTLESS_I;
      if (c == LATIN5_CAPITAL_I_WITH_DOT)
	return 'i';
    }
  return tolower (c);
}

wint_t
procps_towupper (wint_t wc)
{
  if (folding == PROCPS_CASE_TURKISH_UTF8 && wc == L'i')
    return 0x130;
  return towupper (wc);
}

wint_t
procps_towlower (wint_t wc)
{
  if (folding == PROCPS_CASE_TURKISH_UTF8 && wc == L'I')
    return 0x131;
  return towlower (wc);
}
