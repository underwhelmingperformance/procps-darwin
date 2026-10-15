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
cannot read, and `pgrep -r` selects no process whose state is unknown, even with
`-r -`.

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

procps-ng's `ps -a` and `ps -d` leave out session leaders, which procps-ng
recognises by a session ID that equals the process ID. `ps` recognises them by
`EPROC_SLEADER`, as for the `s` modifier.

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

`ps`'s `ni` column shows `-` for `SCHED_FIFO` and `SCHED_RR`, as procps-ng shows
it for a real-time process, and the nice value otherwise, including when the
policy is unknown. The nice value is `p_nice` from `kinfo_proc`, which any user
can read. The policy and the Darwin base priority come from `PROC_PIDTASKINFO`,
so without the helper they are unknown for another user's process. procps-ng
prints `-` as the class of an unknown policy. Darwin has no task information for
a zombie, whereas Linux reports the policy that the process had before it
exited. The tools approximate a zombie's policy as `SCHED_OTHER`, which most
processes use, so a zombie that had a real-time policy shows the class `TS`, not
`FF` or `RR`.

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
| `%mem`      | `VmRSS` divided by `MemTotal`                   | `pti_resident_size` divided by `hw.memsize`                    |
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
Linux, as they are in Apple's `ps`. `sz` counts pages of the caller's page size.
Apple silicon has pages of 16 KiB, and Linux's pages usually have 4 KiB.
`MemTotal` leaves out the memory that Linux's kernel reserves, whereas
`hw.memsize` is the installed memory.

The kernel frees a zombie's memory, and Darwin has no task information, threads
or memory regions for a zombie. `vsz`, `rss`, `sz`, `%mem`, `trs`, `drs`,
`tsiz`, `dsiz` and `size` show 0 for a zombie, as on Linux, `nlwp` shows 1, and
the zombie's CPU time comes from `proc_pid_rusage`. Linux keeps a zombie's count
of minor faults, and Darwin does not, so `min_flt` shows `-` for a zombie.

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

Linux reports `start_code` and `end_code` in `/proc/<pid>/stat` as 1 to a caller
that fails ptrace's read-access check, so procps-ng then shows `trs` and `tsiz`
as 0 and `drs` and `dsiz` as the whole virtual size. `ps` does the same when the
helper's access policy denies the effective user and group of `ps` access to the
process's private values, even if the helper has sent the region totals.

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
on Linux. As on Linux, the 15 bytes can end inside a multi-byte character, and
the escaping then replaces each byte of the partial character with `?`.

On Linux, a script that the kernel runs through its `#!` line has the script's
name, whereas Darwin's `p_comm` for such a script is the name of the program
that finally runs it. For a script called `myscript` that starts with
`#!/bin/zsh`, `pgrep -x myscript` therefore selects nothing on macOS, and
`pgrep -x zsh` selects it. A `#!/bin/sh` script has the name `bash` by default,
because `/bin/sh` on macOS is a small program that runs the shell that
`/private/var/select/sh` links to. That link refers to `bash` unless the
administrator has changed it. `pgrep -f myscript` selects the script on both
systems. The tools could approximate Linux's name from the arguments, but macOS
withholds other users' arguments unless the helper reads them, so the name would
depend on whether the helper is running.

Linux also takes the name from the path that `execve` receives, so a program
that starts through a symbolic link has the link's name. Darwin's `p_comm` has
the name of the file that the link resolves to. A `python3` that is a link to
`python3.14`, as Nix installs it, is therefore called `python3.14` on macOS, and
`pgrep -x python3` selects it only on Linux. `pgrep -f python3` selects it on
both systems, because the command line keeps the name that the caller used.

## Command lines that the tools cannot read

Without the helper, macOS withholds the arguments of another user's process. The
tools then use the command line that procps-ng makes for an unreadable
`/proc/<pid>/cmdline`: the command name in brackets, followed by `<defunct>` for
a zombie. `pgrep -a` prints that text, `pgrep -a -Q` prints `-`, and `pgrep -f`
matches the pattern against that text. On Linux, procfs normally lets every user
read every process's command line, so procps-ng prints and matches the real one,
although a `hidepid` mount option can withhold them. With the helper, the tools
can read the command line of every process apart from `kernel_task`.

## Environment

`pgrep --env` reads each process's environment from `KERN_PROCARGS2`. macOS 27
withholds the environment of an Apple platform binary, such as `/bin/sleep` or
`/bin/sh`, from every process except the binary itself, and the tools treat that
environment as unreadable. `pgrep --env` therefore never selects a platform
binary on macOS 27. On Linux, a process's owner can read its environment from
`/proc/<pid>/environ`, so procps-ng selects it.

## Sessions

Darwin's `getsid` fails for a zombie, whereas Linux reports a zombie's session.
A process group is always in one session, so when the tools cannot read a
process's session, they take it from another process in the same process group.
`pgrep -s` and `ps -s` use that session to select the process, and `ps` shows it
in the `sid` and `sess` columns. `ps` reads the identity of every process to
find the other members of the group, even when `-p` or `-q` lists the processes.
The tools find no session for a zombie unless another member of its process
group has a session that the source can read, and `ps` then shows `-`.

## Start times

procps-ng reads a process's start time from Linux as a count of clock ticks
since boot, at 100 ticks a second, so two processes that start in the same tick
have the same start time for `pgrep -n` and `-o`. Darwin records the start time
by the wall clock, in microseconds. The tools subtract the time since boot,
which `mach_continuous_time` gives, from the time of the snapshot to find the
time of boot, then count the ticks between boot and the start. If the wall clock
has been set since a process started, the process's start tick is offset by the
size of the change, but never to before tick 0.

`ps`'s start columns, `stime`, `start_time`, `start`, `bsdstart` and `lstart`,
show the start time from `kinfo_proc` in whole seconds. procps-ng adds the start
tick to the time of boot in `/proc/stat`, which has whole seconds, so on Linux
these columns can be a second earlier than the real start. `etime`, `etimes`,
`%cpu`, `c` and `cp` count clock ticks since boot, as procps-ng does.

## Page faults

procps-ng's `maj_flt` and `min_flt` columns show Linux's counts of the page
faults that read from disk and of the others. Darwin's `PROC_PIDTASKINFO` counts
every fault in `pti_faults`, including copy-on-write faults, and its page-ins in
`pti_pageins`. A page-in approximates a fault that reads from disk: Darwin also
counts a page-in for the first fault on a page that an earlier clustered read
brought into memory. The tools subtract the page-ins from the faults to
approximate the minor faults:

| Column                        | Linux source       | Darwin source                                                      |
| ----------------------------- | ------------------ | ------------------------------------------------------------------ |
| `maj_flt`, `majflt`, `pagein` | `majflt` in `stat` | `pti_pageins`, or `ri_pageins` from `proc_pid_rusage` for a zombie |
| `min_flt`, `minflt`           | `minflt` in `stat` | `pti_faults` minus `pti_pageins`, or `-` for a zombie              |

## Filesystem IDs

Linux keeps a user and a group ID for file access, which are normally the
effective IDs. Darwin has no such IDs, so `fuid`, `fuser`, `fgid`, `fgroup`,
`fsuid`, `fsuser`, `fsgid` and `fsgroup` show the effective IDs from
`kinfo_proc`.

## Signal masks

`ps`'s `caught` and `ignored` columns show `p_sigcatch` and `p_sigignore` from
`kinfo_proc` as procps-ng shows the masks of `/proc/<pid>/status`: 16
hexadecimal digits, or the last 8 when the line has no room for 16. Bit `n - 1`
is set for Darwin's signal `n`, so the masks use Darwin's numbers, as `pkill`
does. `SIGUSR1`, for example, is bit 29 on Darwin and bit 9 on Linux. Darwin
also counts `SIGURG`, `SIGCHLD`, `SIGIO`, `SIGWINCH` and `SIGINFO`, whose
default action is to ignore them, as ignored, whereas Linux counts only the
signals that the process has set to `SIG_IGN`. XNU keeps both kinds in one mask,
from which it fills `p_sigignore`, and no call that the tools use reports a
process's signal actions, so the tools cannot tell a `SIG_IGN` of one of these
signals from its default action. `SIGCONT`, whose default action also ignores it
when the process is not stopped, is the exception. XNU never sets its bit in
that mask, even when the process sets `SIGCONT` to `SIG_IGN`, so `ignored` never
shows `SIGCONT`, whereas Linux shows it for a process that ignores it.
`kinfo_proc` reports 0 for every pending and blocked signal, so `pending`,
`blocked` and the other masks of pending and blocked signals show `-`.

## Terminal names

`ps`'s `tty`, `tt` and `tname` columns show the name of the controlling terminal
in `/dev`, from `devname_r`, such as `ttys001` or `console`. Linux's terminals
have names such as `pts/0` and `tty1`. `tty4` removes `tty` from the start of
the name, as procps-ng removes it, so it shows `s001`. A process without a
controlling terminal shows `?`. Linux numbers no terminal 0, whereas Darwin's
console has the device number 0, so the tools decide whether a process has a
terminal from `kinfo_proc` and not from the device number.

## Sorting

procps-ng sorts text with `strcoll`, which follows the `LC_COLLATE` locale. `ps`
compares the bytes, as `strcoll` does in the C locale, so in other locales the
order of names and command lines can differ from Linux's. A value that the
source could not read, such as another user's CPU time without the helper, sorts
as a value lower than every other value: first in ascending order and last in
descending order.

## Character widths

`ps` cuts a column's text to the screen cells that the column has, as procps-ng
does. In a UTF-8 locale, procps-ng replaces each character that glibc's
`iswprint` rejects with `?`, and counts the cells of the others with glibc's
`wcwidth`. Darwin's classes and widths differ from glibc's, and Darwin's
`iswprint` rejects characters such as emoji. `ps` therefore uses tables from
glibc 2.44's locale data, which `scripts/generate-glibc-ctype` converts: glibc's
`print` class and the widths of its UTF-8 character map. For 8976 code points
sampled across Unicode, `ps` and procps-ng in the reference container print the
same text and pad it to the same width. The reference container uses the glibc
of the flake's nixpkgs. The flake's `glibc-ctype` check regenerates the tables
from that glibc's source, with nixpkgs' patches applied, and fails when they
differ from the committed file, so an update of nixpkgs that changes glibc's
version or locale data needs new tables.

## `ps --info`

procps-ng's `--info` prints the device number of its terminal as Linux's major
and minor numbers, and its page size. `ps` uses Darwin's layout of a device
number: the major number is the top 8 bits, and the minor number is the low 24
bits. Without a controlling terminal, both numbers are 0, as in procps-ng. On
Apple silicon the page size is 16384. procps-ng prints the effective user ID
with `%d`, so an ID above 2147483647 is negative, and `ps` prints it the same
way: `nobody`'s ID, 4294967294, is `-2`. procps-ng also prints the glibc and GCC
versions that it was compiled with, which `ps` leaves out.

## `kernel_task`

`kernel_task` has process ID 0, and even root cannot read its executable path,
its command line or its memory regions. Where `top` shows it, its `exe` column
therefore shows `-`, its command line shows `[kernel_task]` as for a Linux
kernel thread, and the region columns show `-`.

Linux does not show its idle tasks, which have process ID 0, in `/proc`, so
procps-ng never selects a process with ID 0. `ps`, `pgrep` and `pkill` leave out
`kernel_task`. `kill(2)` reads the process ID 0 as the caller's process group,
so a script that passes `pgrep`'s output to `kill` would otherwise signal its
own process group, and so would `pkill`. `Pid::send_signal` also refuses pid 0.

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

procps-ng's `ps` installs a handler for each signal from 1 to 31 except
`SIGINT`, `SIGQUIT`, `SIGKILL`, `SIGSTOP`, `SIGCONT`, `SIGTSTP`, `SIGTTOU`,
`SIGPROF`, `SIGWINCH`, `SIGURG` and `SIGCHLD`. `ps` installs the same handler
for the signals that both systems have. Darwin's `SIGEMT` and `SIGINFO` keep
their default actions, so `SIGINFO`, which the terminal sends for Control-T,
does not end `ps`. The handler's message gives Darwin's number and procps-ng's
name for the signal, such as `Signal 30 (USR1) caught by ps (4.0.7).`

For each signal that it reports as a bug, the handler then restores the signal's
default action and sends the signal again, and for most of these signals the
default action ends `ps`. On Linux the default action of `SIGIO` ends a process
too, so procps-ng's `ps` dies of it after the message and the bug report. Darwin
discards `SIGIO` by default and has no other way to end a process with it, so
`ps` prints the same two lines and exits with status 1. `SIGTTIN`, the only stop
signal with the handler, stops `ps` on both systems until `SIGCONT`, and `ps`
then exits with status 1. In an orphaned process group, both systems discard
`SIGTTIN`, and `ps` exits with status 1 at once.

procps-ng's handler writes the output that `ps` has buffered in two of its
cases. On `SIGHUP`, `SIGUSR1` and `SIGUSR2`, it ends with `exit`, and
`close_stdout` then writes the buffer and reports a failed write. For the
signals that it reports as a bug, `error_at_line` writes the buffer after the
first line of the message and before the bug report. On `SIGTERM` and `SIGPIPE`,
it writes nothing more. `ps` writes no buffered output from its handler in any
case, because the handler cannot reach the buffer soundly. The buffer belongs to
`procps::output::Output`, which moves the buffered bytes out of it before it
calls `write(2)`, so the bytes of an interrupted write are not in the buffer.
The code that the signal interrupts can also be changing the buffer, and a
handler must not read memory in that state. Reporting a failed write would also
need `close_stdout`'s messages, which `ps` formats with functions that are not
async-signal-safe.

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
locale of that name. Darwin reports the codeset of that locale as an empty
string, and with an empty codeset `iconv` decodes no single byte above 0x7f, so
the tools escape every byte outside ASCII.

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

procps-ng's `ps` decides how to read an argument from its first bytes, with
glibc's `isalpha` and, after `--`, `iswalpha`. Both follow the `LC_CTYPE`
locale. In the C locale and in UTF-8 locales, `isalpha` accepts only the ASCII
letters, and in a UTF-8 locale `iswalpha` accepts letters outside ASCII too. So
`ps --é` is an unknown long option in `C.UTF-8` and garbage in `C`.

`ps` approximates these classes. In a UTF-8 locale, it decodes the first
character and accepts it if Unicode classifies it as alphabetic. In another
encoding, it decodes the first byte on its own, so the first byte of a character
that has several bytes, as in Shift JIS, is not a letter. glibc's classes also
depend on the locale, and not only on its encoding.

## Regular expressions

procps-ng compiles the pattern when it starts to select processes. With `-A`, it
reads the process table earlier, while it parses the options, to find its
ancestors, so a failure to read the table comes before a regex error. The tools
read the table after they compile the pattern. With an invalid pattern and an
unreadable process table, `pgrep -A` therefore reports the regex error.

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

## Escaped text

procps-ng replaces each byte of a character that is unsafe on a terminal, and
each byte of an invalid sequence, with `?` in command names, thread names,
command lines and environment strings, before it matches or prints them. In a
UTF-8 locale, it keeps valid characters other than control and private-use
characters. In any other locale, it keeps the bytes that `isprint` accepts.
glibc's locales classify every character other than a control character as
printable, whereas Darwin's `isprint` also accepts bytes that encode no
character, such as 0xae in ISO 8859-7. The tools therefore decode each of the
256 byte values with `iconv` in the locale's encoding, and keep a byte that
decodes to a character other than a control character. If `iconv` does not know
the encoding, the tools keep the printable characters of ASCII. Apple's `iconv`
and glibc's character maps disagree about some bytes: in ARMSCII-8, about 0xa1,
0xa4, 0xa5, 0xa9, 0xab, 0xac and 0xff, and in GBK, about 0x80. For those bytes,
the tools follow `iconv`.

## Threads

With `-w`, procps-ng's `pgrep` selects and lists each thread of a process. Linux
calls each thread a task. `pgrep` lists Darwin's threads in place of Linux's, in
ascending order of thread ID within each process. The thread IDs come from
`proc_pidinfo` with `PROC_PIDLISTTHREADIDS`, and each thread's name and run
state from `pth_name` and `pth_run_state` of `PROC_PIDTHREADID64INFO`.

- A thread's ID is its 64-bit Darwin thread ID, which is unique among threads. A
  Linux thread ID is a pid, and the main thread's ID equals the process ID, so
  `kill $(pgrep -w name)` signals the threads' processes on Linux. Darwin's
  thread IDs are not pids, but a thread created soon after boot can have an ID
  that equals the pid of an unrelated process, which `kill` would then signal.
  Do not pass the output of `pgrep -w` to `kill` on macOS.
- Darwin does not mark a process's main thread. Darwin assigns thread IDs in
  increasing order, so the main thread, the first thread of the process, has the
  lowest ID. The tools treat that thread as the main thread. Once the main
  thread has exited, the thread with the next lowest ID is treated as the main
  thread, whereas Linux keeps the exited main thread as a zombie task.
- On Linux, a process's name is its main thread's name, so a main thread that
  sets its own name renames the process, and `pgrep` matches the new name. The
  tools give the process and its main thread the process's name from `p_comm`,
  and `pgrep` does not match a name that the main thread set. Many macOS
  programs set a name for their main thread. Rust's runtime gives it the name
  `main`, and on one macOS 27 system, 265 of 1090 processes had a named main
  thread, most often `MainThread` or `main`. If the tools used those names,
  `pgrep` would not select these processes by their program's name.
- Any other thread has the name that it set with `pthread_setname_np`, cut to 15
  bytes, the longest name that Linux keeps. A thread that has not set a name has
  the process's name, because a Linux thread inherits the name of the thread
  that created it, which is usually the process's name.
- A thread's state letter comes from its run state: a running thread is `R`, a
  thread in an uninterruptible wait is `D`, and any other thread is `S`, as for
  a process under [Process state][process-state]. Every thread of a zombie or a
  stopped process has the process's letter.
- Darwin reports no start time for a thread, so `-n`, `-o` and `-O` use the
  process's start time for each of its threads. On Linux, each thread has its
  own start time, so `pgrep -w -n` selects the newest thread on the system, and
  `pgrep -w -o` the oldest. The tools select the thread with the highest ID in
  the newest process, or the thread with the lowest ID in the oldest process.
- procps-ng compares each thread's ID with the pid of `pgrep` itself and, with
  `-A`, with the pids of `pgrep`'s ancestors, so on Linux it leaves out only the
  main threads of those processes. The tools leave out the main thread of each
  ancestor too, and every thread of their own process. procps-ng's `pgrep` has
  one thread, so on Linux the tool has no other thread to leave out.
- When the tools cannot read the threads of a process, or the process has none,
  they list the process as one task whose ID is the pid, as a single-threaded
  process is on Linux. Without root or the helper, Darwin refuses
  `PROC_PIDLISTTHREADIDS` for another user's process, so `pgrep -w` lists each
  such process once, by its pid, and `pgrep -w -c` counts it once. The output of
  `pgrep -w` can therefore mix pids and Darwin thread IDs. With procfs's default
  permissions, procps-ng lists every thread, because every user can read
  `/proc/<pid>/task`, although a mount option such as `hidepid` can withhold
  other users' processes. The helper reads the threads of every process.
- `pkill` accepts `--lightweight` too. On Linux, `pkill` then sends the signal
  to each thread's ID, and the kernel delivers the signal to the thread's
  process. Darwin's thread IDs are not pids, so `pkill` sends the signal to each
  thread's process instead. As on Linux, `pkill` sends the signal to a process
  once for each of its threads, and `-e` reports the thread IDs.

[process-state]: #process-state

## Pid reuse before a signal

procps-ng's `pkill` selects the processes, then opens a pidfd for each pid just
before it signals the process. The pidfd refers to whichever process has the pid
at that moment, so procps-ng signals a process that took the pid after the
selection. Darwin has no pidfds. To avoid signalling such a process, `pkill`
reads each process's start time again just before it sends the signal. If the
process has exited or its start time differs from the snapshot's, `pkill` leaves
it out. `pkill` is therefore stricter than procps-ng, but another process can
still take the pid between the check and the signal.

If the read fails for another reason, `pkill` prints `killing pid <pid> failed`
with a description of the error, in the same form as for a failed signal, and
does not signal the process. procps-ng has no such read, so it prints this
message only when its signalling call fails, whether that call is
`pidfd_send_signal`, `kill(2)` or `sigqueue`.
