<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# Linux concepts on macOS

This file lists each place where the tools approximate a Linux concept with
Darwin data, and where each value comes from.

## Largest pid

procps-ng reads `/proc/sys/kernel/pid_max` and pads pid columns to the number of
digits in `pid_max - 1`. Without that file, it uses five digits.

Darwin has no `pid_max` setting. XNU allocates pids up to `PID_MAX`, which is
99999 in [`bsd/sys/proc_internal.h`][proc_internal], so the tools behave as
procps-ng does with `pid_max` set to 100000: pid columns are five characters
wide. The reference harness presents the same value to procps-ng when it
generates the golden files.

[proc_internal]:
  https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_internal.h
