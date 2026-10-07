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
Darwin keeps 16 bytes in `p_comm`. The tools keep the first 15 bytes of
`p_comm`, so `ps`'s `comm` column, `pgrep`'s matching and its warning behave as
on Linux. As on Linux, the 15 bytes can end inside a multi-byte character.

## `kernel_task`

`kernel_task` has process ID 0, and even root cannot read its executable path,
its command line or its memory regions. Its `exe` column therefore shows `-`,
its command line shows `[kernel_task]` as for a Linux kernel thread, and the
region columns show `-`.

## Signals

`pkill` accepts procps-ng's signal names and numbers. A name selects the Darwin
signal with that name, and a number other than 32 to 64 is sent unchanged.
Darwin assigns different numbers to several signals: for example, `pkill -10`
sends `SIGUSR1` on Linux and `SIGBUS` on Darwin. A script that uses names, such
as `pkill -USR1`, sends the same signal on both systems. Numbers 1 to 6, 8, 9,
11, 13 to 15, 21, 22 and 24 to 28 refer to the same signal on both.

Darwin has no `SIGCLD`, and defines `SIGPOLL` only in strict POSIX mode, with
the number of `SIGEMT`. On Linux they are other names for `SIGCHLD` and `SIGIO`,
so the tools accept `CLD` and `POLL` and send Darwin's `SIGCHLD` and `SIGIO`.

Darwin lacks `SIGPWR`, `SIGSTKFLT` and the real-time signals. The tools
recognise their names as procps-ng does on Linux: `PWR`, `STKFLT`, `RTMIN`,
`RTMAX`, `RTMIN+n` and `RTMAX-n`. They also read the numbers 32 to 64, which are
Linux's real-time signals, as real-time signals. `pkill` then exits with status
2, as for the other options for Linux concepts, unless a later `--signal`
replaces the signal. `pgrep` uses the signal only for `-H`, and exits with
status 2 only then.

Darwin's `SIGEMT` and `SIGINFO`, which Linux lacks, are accepted as the values
`EMT` and `INFO` of `--signal`, whereas procps-ng on Linux reports them as
unknown signals. procps-ng on Linux reads `pkill -emt` as `-e -m -t` and
`pkill -info` as `-i -n -f -o`, so the tools also read `-emt` and `-info` as
options.

## Error descriptions

Some of procps-ng's messages end with the C library's description of an error
number, such as `Unable to open pidfile: No such file or directory`. For the
error numbers that both systems have, the tools print glibc's descriptions.
Darwin's description differs for 25 of them, such as `ENXIO`, which Darwin
describes as `Device not configured` and glibc as `No such device or address`.
Linux gives `ENOTSUP` and `EOPNOTSUPP` one number, which glibc describes as
`Operation not supported`, so the tools print that for both. An error number
that only Darwin has, such as `EAUTH`, keeps Darwin's description.

For error number 0, glibc's description is `Success` and Darwin's is
`Undefined error: 0`. When `-L` finds no lock on the pid file because both lock
calls succeeded, procps-ng prints the description of `errno`. A successful call
leaves `errno` unchanged, so the value is whatever an earlier library call left.
In the reference harness, in the C locale, the value is 0, so procps-ng prints
`Success`. In C.UTF-8, `setlocale` has left `ENOENT`, so procps-ng prints
`No such file or directory`. The tools print `Success` in every locale.

## Closed descriptors

procps-ng's `main` calls `epoll_create` before anything else, so its epoll
descriptor takes the lowest descriptor that was closed when it started. Linux
refuses to open an epoll descriptor through `/dev/fd`, so
`pgrep -F /dev/stdin <&-` reports `No such device or address`, and the path of
any other descriptor that was closed at start-up does not exist. The tools give
the same errors. Linux's procfs accepts only the canonical decimal form of a
descriptor's number, so `/dev/fd/03` and `/dev/fd/+3` do not exist on Linux, and
the tools report `No such file or directory` for them too.

darwin-proc records which of the descriptors 0 to 255 were closed at start-up.
For a closed descriptor above 255, `pgrep -F /dev/fd/<n>` reports Darwin's
`Bad file descriptor`. Linux reports `No such device or address` if the
descriptor is procps-ng's epoll descriptor, and `No such file or directory`
otherwise.

## Locks on a pid file

With `-L`, procps-ng checks that another process has locked the pid file with
`flock` or `fcntl`. Linux locks a pipe as it locks a file, so for a pid file on
standard input from a pipe, both lock calls succeed. Darwin's `flock` fails on a
pipe or a socket with `ENOTSUP`, and `fcntl` with `EBADF`. For those file types,
the tools treat these errors as granted locks, as Linux grants them.

## Message language

procps-ng translates its messages with gettext, and glibc translates the
messages of `getopt_long` and the descriptions of error numbers. Both follow the
locale. The tools have no translations, and print every message in English, as
procps-ng and glibc do in the C locale.

## Locales

glibc normalises the codeset in a locale name, so `C.utf8` and
`en_US.ISO-8859-1` refer to the locales that Darwin calls `C.UTF-8` and
`en_US.ISO8859-1`. The tools use the installed Darwin locale that has the same
language and modifier, and a codeset that is the same after glibc's
normalisation.

On Linux, glibc has the `C` and `POSIX` locales built in, and any other locale
exists only if the system has generated it. Distributions generate locales from
the names in glibc's `localedata/SUPPORTED` file, and an administrator can
generate others with `localedef`. That file has no `en_US.ISO-8859-15` or
`ja_JP.SJIS`, although Darwin installs both. darwin-proc lists the names in that
file for the languages that Darwin installs, and the tools fail to set any other
name with a codeset, as procps-ng fails on a system that has generated only the
locales in that file.

glibc's `setlocale` also reads the aliases in glibc's `locale.alias`, such as
`german` for `de_DE.ISO-8859-1` and `no_NO.ISO-8859-1` for `nb_NO.ISO-8859-1`,
and compares a name with an alias without regard to case. The tools read the
same aliases.

A locale name without a codeset can refer to a different encoding on each
system. Darwin's `en_US` is a UTF-8 locale, and Darwin reports its codeset as an
empty string, whereas glibc's `en_US` uses ISO 8859-1. For such a name, the
tools use the Darwin locale with glibc's codeset, here `en_US.ISO8859-1`.
darwin-proc lists glibc's codeset for each locale name that Darwin installs
without a codeset. glibc lacks `fa_AF`, `se_FI` and `sr_YU`, so procps-ng and
the tools both fail to set these locales and stay in the C locale. For `ar_AE`,
`ar_EG`, `ar_JO`, `ar_MA`, `ar_QA`, `ar_SA`, `en_PH`, `es_CR`, `ga_IE` and
`he_IL`, Darwin has no locale with glibc's codeset, and the tools use Darwin's
locale of that name.

Darwin also installs a locale called `UTF-8`, which glibc lacks, and the
Terminal app on macOS sets `LC_CTYPE=UTF-8`. procps-ng would run in the C locale
for that setting and escape every byte outside ASCII. The tools use Darwin's
`UTF-8` locale instead, so that names and command lines outside ASCII stay
readable in the terminal.

glibc also has names with a modifier but no codeset, such as `de_DE@euro` and
`sr_RS@latin`. The tools use glibc's codeset for such a name. Darwin lacks most
of these modifiers, so the tools leave out a modifier that Darwin lacks:
`de_DE@euro` is `de_DE.ISO8859-15`. A locale with a modifier can collate
differently from the locale without it.

glibc has six categories that Darwin lacks: `LC_PAPER`, `LC_NAME`, `LC_ADDRESS`,
`LC_TELEPHONE`, `LC_MEASUREMENT` and `LC_IDENTIFICATION`.
`setlocale(LC_ALL, "")` fails in procps-ng when the environment gives one of
them a locale that does not exist, so the tools check those variables too. They
check that Darwin installs the locale, which gives glibc's answer for every
locale that both systems have.

The tools also fail to set a locale that glibc has and Darwin does not install,
such as `en_DK.UTF-8`. They then stay in the C locale and escape every byte
outside ASCII. On Linux, procps-ng uses the locale if the system has generated
it. The same applies to the categories that only glibc has, so
`LC_PAPER=en_DK.UTF-8` has this effect too. macOS's SSH server accepts the
variables `LANG` and `LC_*`, so a session from Linux can bring such a setting
with it.

procps-ng compares signal names with glibc's `strcasecmp`, which folds case as
the `LC_CTYPE` locale does. glibc's Turkish locales pair `i` with `İ` and `I`
with `ı`. In `tr_TR.UTF-8`, `i` and `I` therefore do not match, so `int` is not
a signal and `pkill -int` is read as the options `-i`, `-n` and `-t`. In
`tr_TR.ISO-8859-9`, `ı` (0xFD) matches `I`. The tools compare signal names with
glibc's rules for the locale. glibc gives `tr_TR`'s rules to other locales, such
as `az_AZ` and `ku_TR`, and has rules of its own for `mt_MT`, but Darwin
installs none of them.

## Regular expressions

procps-ng compiles patterns with glibc's `regcomp` and matches them with
`regexec`. Darwin's extended syntax lacks glibc's extensions, such as
back-references, `\w` and `\<`. Darwin's enhanced syntax has most of them, but
it gives escapes such as `\d` meanings that glibc's syntax lacks, and Darwin's
engine misses matches for patterns such as `(^|\<)sshd`. The tools therefore use
glibc 2.44's regular expression code, which darwin-proc compiles from
`crates/darwin-proc/glibc-regex`, and they print glibc's error messages.

Outside glibc, the code has no access to glibc's locale data, so it uses
Darwin's functions for character classes, case conversion and multi-byte
characters. Two differences follow in locales other than C:

- glibc reads ranges, equivalence classes and collating elements by the
  collation rules of the `LC_COLLATE` locale. glibc has no rules for C, POSIX,
  C.UTF-8 and the other locales whose names start with `C.`. In those locales,
  glibc compares a character of the text by its code point, and a range end by
  its byte value, so in C.UTF-8 a bracket expression whose ends are the bytes
  0xE9 and 0xEF matches `é` (U+00E9). glibc rejects a range with a multi-byte
  end, such as `[à-ü]` in C.UTF-8, with `Invalid collation character`. A change
  to `build_range_exp` makes the tools behave in the same way in those locales.

  The code outside glibc has no collation rules. In the other locales, the tools
  compare the wide characters that Darwin's `btowc` and `mbrtowc` return, which
  are code points in UTF-8 and ISO 8859-1, and accept an equivalence class or a
  collating element only for a one-byte character. In `en_US.UTF-8` and
  `en_US.ISO-8859-1`, these expressions match different characters:

  | Expression | glibc matches      | The tools match                                            |
  | ---------- | ------------------ | ---------------------------------------------------------- |
  | `[a-z]`    | `e`, `é`, `ð`, `z` | `e`, `z`                                                   |
  | `[à-ü]`    | `e`, `é`, `ð`      | `é`, `ð`                                                   |
  | `[[=e=]]`  | `e`, `é`, `E`, `É` | `e`                                                        |
  | `[[=é=]]`  | `e`, `é`, `E`, `É` | `é` in ISO 8859-1; in UTF-8, `Invalid collation character` |

- Character classes such as `[[:alpha:]]` and `\w`, and the case conversion of
  `-i`, follow Darwin's tables for the locale. For characters outside ASCII,
  these can differ from glibc's. The exception is the Turkish `i`: glibc's
  Turkish locales pair `i` with `İ` and `I` with `ı`, whereas Darwin's pair `i`
  with `I`. The tools fold these four letters as glibc does, so in
  `tr_TR.UTF-8`, `pgrep -i INIT` does not select `init`.
