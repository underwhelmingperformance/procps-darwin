<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# Linux concepts on macOS

This file lists each place where the tools approximate a Linux concept with
Darwin data, and where each value comes from. `procps-core`'s `linux` module
implements the per-process mappings.

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

## Process state

procps-ng copies the state letter from `/proc/<pid>/stat` and appends modifiers.
Darwin's `p_stat` in `kinfo_proc` is `SRUN` for almost every live process,
whether it is running or asleep, so the letter comes from several sources:

| Letter | Darwin source                                                                      |
| ------ | ---------------------------------------------------------------------------------- |
| `Z`    | `p_stat` is `SZOMB`                                                                |
| `T`    | `p_stat` is `SSTOP`                                                                |
| `t`    | `p_stat` is `SSTOP` and `p_flag` has `P_TRACED`                                    |
| `R`    | a thread's `pth_run_state` is `TH_STATE_RUNNING`                                   |
| `D`    | no thread is running, and a thread's `pth_run_state` is `TH_STATE_UNINTERRUPTIBLE` |
| `S`    | no thread is running or in an uninterruptible wait                                 |
| `-`    | the tools could read neither the threads nor the task information                  |

Without the threads, the number of running threads in `PROC_PIDTASKINFO` decides
between `R` and `S`, and `D` cannot appear. Another user's threads and task
information are readable only through the helper, so without it only `Z`, `T`
and `t` are available for another user's process. Linux has no letter for an
unknown state. The tools show `-`, as they do for any value that the caller
cannot read.

The modifiers follow procps-ng's order:

| Modifier | Darwin source                                                       |
| -------- | ------------------------------------------------------------------- |
| `<`      | `p_nice` is negative                                                |
| `N`      | `p_nice` is positive                                                |
| `s`      | `e_flag` has `EPROC_SLEADER`                                        |
| `l`      | the process has more than one thread                                |
| `+`      | `e_tpgid`, the terminal's foreground process group, equals `e_pgid` |

Linux's `L` marks a process with locked pages. Darwin does not report locked
pages separately from other wired memory, so the state never has `L`.

## Priority, nice and scheduling policy

procps-ng computes `pri`, `opri`, `priority`, `rtprio`, `ni`, `class` and
`top`'s `PR` and `NI` from four Linux values: the `priority` field of
`/proc/<pid>/stat`, the nice value, the real-time priority and the scheduling
policy. The tools derive them as follows:

| Darwin policy (`pti_policy`) | Linux policy  | `priority`                      | Real-time priority                 |
| ---------------------------- | ------------- | ------------------------------- | ---------------------------------- |
| `POLICY_TIMESHARE`           | `SCHED_OTHER` | 20 plus `p_nice`                | 0                                  |
| `POLICY_FIFO`                | `SCHED_FIFO`  | -1 minus the real-time priority | `pti_priority`, limited to 1 to 99 |
| `POLICY_RR`                  | `SCHED_RR`    | -1 minus the real-time priority | `pti_priority`, limited to 1 to 99 |
| unknown                      | none          | 20 plus `p_nice`                | none                               |

The nice value is `p_nice` from `kinfo_proc`, which any user can read. The
policy and the Darwin base priority come from `PROC_PIDTASKINFO`, so without the
helper they are unknown for another user's process. procps-ng prints `-` as the
class of an unknown policy.

Darwin has no separate real-time priority, so a fixed-priority process shows its
Darwin base priority as its real-time priority. Darwin's priorities run from 0
to 127, with 31 as the default for timesharing, and Linux's real-time priorities
run from 1 to 99, so the value is limited to that range.

## Process flags

procps-ng's `f` column shows `(flags >> 6) & 7` of Linux's `PF_*` flags in
octal, and `top`'s `Flags` field shows the whole word. Two of the three bits in
that range have equivalents in Darwin's accounting flags, which `kinfo_proc`
reports in `p_acflag`. Any user can read them.

| Linux flag                                           | Darwin source |
| ---------------------------------------------------- | ------------- |
| `PF_FORKNOEXEC` (0x40, "forked but didn't exec")     | `AFORK`       |
| `PF_SUPERPRIV` (0x100, "used super-user privileges") | `ASU`         |

The third bit, `PF_MCE_PROCESS` (0x80), has no Darwin equivalent, so `f` is 0,
1, 4 or 5.
