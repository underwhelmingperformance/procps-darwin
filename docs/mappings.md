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

## Memory and swap in `top`'s summary

procps-ng computes `top`'s memory line from Linux's `/proc/meminfo`. Darwin's
`HOST_VM_INFO64` counts pages in other categories, so the tools compute each
figure from the nearest Darwin categories:

| Figure       | Linux source                           | Darwin source                             |
| ------------ | -------------------------------------- | ----------------------------------------- |
| `total`      | `MemTotal`                             | `hw.memsize`                              |
| `free`       | `MemFree`                              | free pages, without the speculative pages |
| `buff/cache` | `Buffers`, `Cached` and `SReclaimable` | file-backed and purgeable pages           |
| `avail Mem`  | `MemAvailable`                         | free, file-backed and purgeable pages     |
| `used`       | `MemTotal` minus `MemAvailable`        | the total minus the available memory      |

XNU counts the speculative pages, which it reads ahead of use, as file-backed
pages too, so the available memory does not add them again. The file-backed and
purgeable pages together are what Activity Monitor shows as "Cached Files". As
in procps-ng, the available memory falls back to the free memory when the sum of
the free, file-backed and purgeable pages is 0 or larger than the total. The
free, used and `buff/cache` figures do not add up to the total.

The swap line uses `vm.swapusage`: its total, its available space as `free`, and
the difference as `used`.

## Processor states in `top`'s summary

procps-ng computes `top`'s CPU line from the ticks in Linux's `/proc/stat`
between two refreshes. Darwin's `host_processor_info` reports four states for
each processor:

| State                  | Darwin source                                                     |
| ---------------------- | ----------------------------------------------------------------- |
| `us`                   | `CPU_STATE_USER`, which includes the user time of niced processes |
| `sy`                   | `CPU_STATE_SYSTEM`                                                |
| `id`                   | `CPU_STATE_IDLE`                                                  |
| `ni`                   | `CPU_STATE_NICE`, which Darwin always reports as 0                |
| `wa`, `hi`, `si`, `st` | none, so 0                                                        |

Darwin's tick counters are 32 bits wide and wrap, so each difference is taken
modulo 2³².

## Process sizes

procps-ng computes `ps`'s size columns from `/proc/<pid>/stat`,
`/proc/<pid>/status` and `/proc/<pid>/smaps_rollup`. The tools use these Darwin
sources:

| Column      | Linux source                                    | Darwin source                                                  |
| ----------- | ----------------------------------------------- | -------------------------------------------------------------- |
| `vsz`, `sz` | `VmSize`                                        | `pti_virtual_size` from `PROC_PIDTASKINFO`                     |
| `rss`       | `VmRSS`                                         | `pti_resident_size` from `PROC_PIDTASKINFO`                    |
| `trs`       | the code segment, `end_code` minus `start_code` | the executable regions that are not submaps                    |
| `drs`       | the virtual size minus the code segment         | the virtual size minus `trs`                                   |
| `size`      | `VmData` plus `VmStk`                           | the private writable regions that are not submaps              |
| `uss`       | private clean and dirty pages in `smaps_rollup` | the private resident pages of the regions that are not submaps |
| `pss`       | `Pss` in `smaps_rollup`                         | none, so `-`                                                   |

Linux's code segment is the main executable's text. Darwin does not report the
segment's bounds, so `trs` counts every executable region of the process except
the submaps, which contain the shared cache of system libraries. It therefore
includes the text of libraries outside the shared cache and of generated code.

Linux's `VmData` counts only private writable mappings, so `size` leaves out the
regions that Darwin reports as shared.

On Darwin, the virtual size of a process includes the shared cache and large
reservations of address space, so `vsz`, `sz` and `drs` are much larger than on
Linux, as they are in Apple's `ps`.

The columns that come from regions, `trs`, `drs`, `size` and `uss`, need a walk
of the memory regions. The walk is slow, and only the helper can do it for
another user's process. The helper sends the regions only to the owner and to
root, in the same way that Linux lets only them read `smaps_rollup`, whose
private clean and dirty sizes make up `uss`. It sends the executable and private
writable totals to every user, in the same way that Linux lets every user read
`VmData` and `VmStk` in `status`, which `size` uses, and the text and data sizes
in `statm`, which `top` uses. Darwin does not report how many processes share
each resident page, so the tools cannot compute a proportional set size, and
`pss` shows `-`.

Linux hides `start_code` and `end_code` in `/proc/<pid>/stat` from a caller that
fails ptrace's read-access check, and procps-ng then shows `trs` as 0 and `drs`
as the whole virtual size. `ps` will do the same for a process whose private
values the caller may not read.

## Executable path

Linux guards `/proc/<pid>/exe` with ptrace's read-access check, so procps-ng's
`exe` column shows `-` for a process that the caller may not read. macOS gives
every user the path through `proc_pidpath`, and the helper passes it on, but
`ps` will show `-` in the same cases as Linux.

## Command names

Linux keeps 15 bytes of a process's command name, and `pgrep` warns when a
pattern without `-f` is longer than that, because the pattern cannot match.
Darwin keeps 16 bytes in `p_comm`. The tools cut `p_comm` to 15 bytes, at a
character boundary, so `ps`'s `comm` column, `pgrep`'s matching and its warning
behave as on Linux.

## `kernel_task`

`kernel_task` has process ID 0, and even root cannot read its executable path,
its command line or its memory regions. Its `exe` column therefore shows `-`,
its command line shows `[kernel_task]` as for a Linux kernel thread, and the
region columns show `-`.
