/*
 * SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

#ifndef PROCPS_DARWIN_COLLATION_H
#define PROCPS_DARWIN_COLLATION_H

#include <stdbool.h>

/* Whether glibc would have collation rules for a locale with the name of the
   calling thread's LC_COLLATE locale.  glibc has none for C, POSIX and
   C.UTF-8.  The function treats every name that starts with "C." as
   C.UTF-8.  */
bool procps_collation_has_rules (void);

#endif
