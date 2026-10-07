/*
 * SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#include <locale.h>
#include <string.h>
#include <xlocale.h>

#include "collation.h"

bool
procps_collation_has_rules (void)
{
  const char *name = querylocale (LC_COLLATE_MASK, uselocale (NULL));

  return name != NULL
	 && strcmp (name, "C") != 0
	 && strcmp (name, "POSIX") != 0
	 && strncmp (name, "C.", 2) != 0;
}
