<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# procps-darwin plan

## Goal and scope

Reimplement `ps`, `top`, `pgrep` and `pkill` from [procps-ng] 4.0.7 for macOS.
The command lines and output should match procps-ng wherever macOS provides the
underlying data, so that scripts and habits from Linux work unchanged.

- Language: Rust, in a Cargo workspace.
- Licence: GPL-3.0-or-later. procps-ng is GPL-2.0-or-later (its library is
  LGPL-2.1-or-later), so its code can be translated into this project. Before
  any code is ported, its file's licence header is checked, as `AGENTS.md`
  describes.
- Target: macOS 27 on arm64. Other macOS versions and architectures are out of
  scope until someone needs them.
- Installation: the binaries use the procps names (`ps`, `top`, `pgrep`,
  `pkill`) and go in a directory that precedes `/bin` and `/usr/bin` in `PATH`.
  Scripts that call `/bin/ps` or `/usr/bin/top` by absolute path keep Apple's
  versions.
- Privileges: an optional root helper daemon supplies data that macOS withholds
  from unprivileged processes. The tools work without it, with reduced data.
- Darwin extensions: `ps` and `top` gain extra fields for data that only macOS
  has, such as physical footprint and energy.

Out of scope for this slice: the other procps-ng programs (`free`, `kill`,
`pidof`, `pidwait`, `pmap`, `pwdx`, `skill`, `snice`, `slabtop`, `sysctl`,
`tload`, `uptime`, `vmstat`, `w`, `watch`, `hugetop`), psmisc, and Linux
support. `pidwait` shares its implementation with `pgrep` in procps-ng and would
be a small follow-up.

[uutils/procps] is an active Rust rewrite of procps-ng. Its tools read `/proc`,
so on macOS most of them fail: `ps` and `pidof` panic, `pgrep` matches no
processes, and `sysctl` exits with "currently only supports Linux". This project
is separate and is designed around the Darwin APIs. uutils/procps uses the MIT
licence, so its code can still be reused here with attribution.

[procps-ng]: https://gitlab.com/procps-ng/procps
[uutils/procps]: https://github.com/uutils/procps

## Audit: macOS tools compared with procps-ng 4.0.7

Each of the four tools is missing procps-ng options on macOS, and several option
letters have different meanings. All four therefore stay in scope.

The macOS side of this audit comes from the man pages and the binaries on macOS
27.0 (build 26A428). The procps-ng side comes from the 4.0.7 man pages.

### pgrep and pkill

Apple's `pgrep` and `pkill` come from FreeBSD. This is their usage text, and the
man page also documents `-a` and `-I`:

```text
usage: pgrep [-Lfilnoqvx] [-d delim] [-F pidfile] [-G gid]
             [-P ppid] [-U uid] [-g pgrp] [-t tty] [-u euid]
             pattern ...
```

| Status on macOS   | procps-ng options                                                                                                                                                                                  |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Same meaning      | `-d`, `-F`, `-f`, `-G`, `-g`, `-i`, `-L`, `-n`, `-o`, `-P`, `-t`, `-U`, `-u`, `-v`, `-x`, `-<signal>` (pkill)                                                                                      |
| Different meaning | `-a` (procps: print the full command line; macOS: include ancestors), `-l` (procps: PID and process name; macOS: with `-f`, the full argument list), `-q` (procps: `sigqueue` value; macOS: quiet) |
| Missing           | `-A`, `-c`, `-e`, `-H`, `-m`, `-O`, `-p`, `-Q`, `-r`, `-s`, `-w`, `--signal`, `--cgroup`, `--env`, `--ns`, `--nslist`, `--quiet`, every other long option, `-h`, `-V`                              |
| macOS only        | `-I` (confirm before signalling)                                                                                                                                                                   |

Two behaviours also differ. By default, macOS excludes all of `pgrep`'s
ancestors from the matches, whereas procps-ng excludes only `pgrep` itself and
excludes ancestors only with `-A`. macOS accepts several patterns, and procps-ng
accepts one; task 0.3 confirms this against a real procps-ng.

### ps

Apple's `ps` is BSD `ps` with Apple changes. `ps -L` lists 64 keywords;
procps-ng's man page documents 153 format specifiers.

- Missing on macOS: every GNU long option (`--pid`, `--ppid`, `--sid`, `--tty`,
  `--user`, `--group`, `--sort`, `--forest`, `--format`, `--headers`,
  `--no-headers`, `--cols`, `--lines`, `--deselect`, `--cumulative`, `--help`,
  `--info`, `--version`), `-N`, `-q`, `-s`, `-F`, `-H` and BSD `f` (forest),
  `-y`, `-P`, the thread displays `-L`, `-T`, `H` and `m`, the BSD selectors `T`
  and `r`, the modifiers `k`, `n` and `c`, the formats `s`, `v` and `X`, and the
  `PS_FORMAT` and `PS_PERSONALITY` environment variables.
- Option letters with a different meaning: `-C` (procps: select by command name;
  macOS: raw CPU calculation), `-L` (procps: show threads; macOS: list
  keywords), `-T` (procps: show threads; macOS: select processes on this
  terminal), `-M` (procps: security label; macOS: show threads), and `-m`
  (procps: show threads after processes; macOS: sort by memory).
- Keywords with a different meaning: `stime` (procps: start time; macOS: system
  CPU time), `sess` (procps: session ID; macOS: session pointer), and `cpu`
  (procps: processor that the process last ran on; macOS: short-term CPU usage
  factor).
- Apple-only keywords include `pagein`, `nvcsw`, `nivcsw`, `msgsnd`, `msgrcv`,
  `inblk`, `oublk`, `utime` and `xstat`.

### top

Apple's `top` has its own command line, screen layout and interactive commands.
None of procps-ng's options (`-A`, `-b`, `-c`, `-d`, `-E`, `-e`, `-H`, `-h`,
`-i`, `-n`, `-O`, `-o`, `-p`, `-S`, `-s`, `-U`, `-u`, `-V`, `-w`, `-1` and their
long forms) has the procps-ng meaning on macOS, except that `-U` filters by user
in both. Letters such as `-c`, `-d`, `-e`, `-n`, `-o`, `-s`, `-S` and `-u` mean
something else. Apple's `top` has no batch mode with procps-ng's output format
(its `-l` logging mode prints a different format), and it does not read `toprc`.

### How Apple's tools get other users' data

`/bin/ps` and `/usr/bin/top` are setuid root and have the private entitlement
`com.apple.system-task-ports.read`. `/usr/bin/pgrep` is not setuid; it has the
entitlement `com.apple.sysmond.client` and asks the `sysmond` daemon for process
data. Third-party binaries cannot obtain either entitlement. A root helper
daemon, similar in role to `sysmond`, is therefore the way for this project to
show the same data to unprivileged users.

## What macOS exposes

These results come from probe programs run as an unprivileged user on macOS 27.0
against the caller's own shell, `launchd` (pid 1, root), `mds` (root) and
`WindowServer` (`_windowserver`).

| Call                                                                                                                                               | Caller's own process                                                                                                                              | Another user's process                                                                                                                                                                                                                               |
| -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sysctl` `KERN_PROC` (`struct kinfo_proc`)                                                                                                         | Succeeds                                                                                                                                          | Succeeds. pid, ppid, pgid, tpgid, terminal device, real, effective and saved IDs, nice, priority, `p_stat`, `p_flag`, start time, 16-character `p_comm`, caught and ignored signal masks. `p_pctcpu`, `p_estcpu`, `p_rtime` and `p_uticks` are zero. |
| `proc_pidinfo` `PROC_PIDT_SHORTBSDINFO`                                                                                                            | Succeeds                                                                                                                                          | Succeeds                                                                                                                                                                                                                                             |
| `proc_pidpath`                                                                                                                                     | Succeeds                                                                                                                                          | Succeeds                                                                                                                                                                                                                                             |
| `getsid`, `getpriority`                                                                                                                            | Succeed                                                                                                                                           | Succeed                                                                                                                                                                                                                                              |
| `kqueue` `EVFILT_PROC` `NOTE_EXIT`                                                                                                                 | Succeeds                                                                                                                                          | Succeeds                                                                                                                                                                                                                                             |
| `proc_pidinfo` `PROC_PIDTBSDINFO`, `PROC_PIDTASKINFO`, `PROC_PIDLISTTHREADS`, `PROC_PIDVNODEPATHINFO`, `PROC_PIDLISTFDS`, `PROC_PIDREGIONPATHINFO` | Succeed                                                                                                                                           | Fail with `EPERM`                                                                                                                                                                                                                                    |
| `proc_pid_rusage`                                                                                                                                  | Succeeds                                                                                                                                          | Fails with `EPERM`                                                                                                                                                                                                                                   |
| `sysctl` `KERN_PROCARGS2` (argv and environment)                                                                                                   | Succeeds. macOS 27 withholds the environment of Apple's platform binaries, and possibly of other processes, from every process except themselves. | Fails with `EINVAL`, even with a buffer of `kern.argmax` bytes                                                                                                                                                                                       |
| `task_name_for_pid`                                                                                                                                | Succeeds                                                                                                                                          | Fails                                                                                                                                                                                                                                                |
| `task_for_pid`                                                                                                                                     | Fails with `KERN_FAILURE` for `/bin/zsh`, a platform binary                                                                                       | Fails                                                                                                                                                                                                                                                |

Without root or the helper, `ps` can therefore show identity, scheduling and
start-time columns for every process, but CPU time, memory, threads, command
arguments, environment, working directory and file descriptors only for the
caller's own processes.

Run as root, the same probe succeeded against `launchd` and `WindowServer`,
which are SIP-protected platform binaries. Every `proc_pidinfo` flavour in the
table, `proc_pid_rusage`, `KERN_PROCARGS2` and `task_name_for_pid` returned
data. Only `task_for_pid` still failed with `KERN_FAILURE`. Root therefore needs
no entitlement to read the data that these tools display, and the helper can
supply every column that Apple's `ps` and `top` show.

`kernel_task` (pid 0) is the exception, even for root. `proc_pidpath` fails with
`ESRCH`, `KERN_PROCARGS2` with `EINVAL`, and `PROC_PIDREGIONPATHINFO` and
`PROC_PIDLISTFDS` with `EPERM`. Its identity, CPU, memory, thread and working
directory data are readable. Task 2.3 decides how `args` and the size columns
appear for it.

No design here depends on task ports. All per-process data comes from
`proc_pidinfo`, `proc_pid_rusage` and `sysctl`.

System-wide data for the `top` summary is available without privileges:
`vm.loadavg`, `kern.boottime`, `hw.memsize`, `host_statistics64` with
`HOST_VM_INFO64` (the counters that `vm_stat` prints), `vm.swapusage`,
`host_processor_info` (per-CPU user, system, idle and nice ticks), and `utmpx`
for the user count.

## Linux concepts with no macOS equivalent

- cgroups, namespaces, SELinux and other LSM labels, capabilities
- systemd units, slices, seats and login sessions; autogroups
- OOM score and adjustment, NUMA nodes, hugetlb pages, LXC and Docker container
  names
- `wchan`, the processor that a task last ran on (`psr`, top's `P`), filesystem
  user and group IDs, and the `eip`, `esp` and `stackp` registers
- CPU time categories `wa`, `hi`, `si` and `st`; the `buffers` memory figure
- `sigqueue(3)` and `process_mrelease(2)`

Some concepts exist on macOS in a different form and need a documented mapping:
process state letters, the `buff/cache` and `avail` memory figures, proportional
set size, scheduling policy names and the `f` flags column.

## Compatibility rules

1. Where macOS provides the data, output matches procps-ng 4.0.7 byte for byte:
   headers, column widths, alignment, number and time formats, error messages
   and exit codes.
2. When a value exists on macOS but the caller is not permitted to read it, the
   column shows `-`.
3. A `ps` format specifier or `top` field for a concept that macOS lacks is
   accepted and shows `-`. Scripts that request such a column keep working.
4. A selection or action option for a concept that macOS lacks (`--cgroup`,
   `--ns`, `--nslist`, `-m`/`--mrelease`, `-q`/`--queue`) is rejected with exit
   status 2 and a message saying that macOS does not support it. Treating it as
   matching every process or no process would make `pkill` signal the wrong
   processes.
5. Darwin extensions use new specifier and field names. A new name must not
   coincide with a procps-ng name or with an Apple `ps` keyword that has a
   different meaning.
6. `docs/mappings.md` lists every approximation, with the Darwin source of the
   value and how it differs from Linux.

## Architecture

### Crates

| Crate            | Contents                                                                                                                                                                                                                         |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `darwin-proc`    | FFI declarations that the `libc` crate lacks, and safe readers for processes, threads, memory regions and system statistics. Errors distinguish a process that has exited, a denied read, and an unsupported call.               |
| `procps-core`    | Domain model, the `ProcessSource` trait and its implementations, the helper protocol and client, process selection shared by all four tools, and formatting helpers (times, sizes, terminal names, cached user and group names). |
| `procps-helperd` | The root helper daemon.                                                                                                                                                                                                          |
| `procps`         | The `ps`, `top`, `pgrep` and `pkill` binaries.                                                                                                                                                                                   |

The `libc` crate (0.2.189) already declares `proc_pidinfo`, `proc_pid_rusage`,
`proc_listallpids`, `proc_bsdinfo`, `proc_taskinfo`, `proc_threadinfo`,
`proc_vnodepathinfo`, `rusage_info_v4`, `host_statistics64`, `vm_statistics64`
and `getutxent`. `darwin-proc` adds `kinfo_proc`, `proc_regionwithpathinfo`,
`rusage_info_v6`, the thread-ID flavours and a parser for the `KERN_PROCARGS2`
buffer. Tests check each added struct's size against the SDK header.

### Data model and sources

Each process record has its identity, which a source always reads, and field
groups: arguments, environment, usage (CPU time, memory and I/O), threads,
memory regions, file descriptors and working directory. Every value apart from
the `kinfo_proc` record is a `Field<T>`, or for the usage group a pair of them.
A `Field<T>` is available, denied, unsupported on macOS for that process, or
failed for another reason. The formatter decides how each case is shown,
following the compatibility rules.

A `SnapshotRequest` lists the field groups that the caller needs.
`ps -o pid,comm` therefore never reads arguments or walks memory regions, and
`top` walks memory regions only when a displayed field needs them.

```rust
trait ProcessSource {
    fn snapshot(&self, request: &SnapshotRequest) -> Result<Snapshot, SourceError>;
}
```

There are three implementations:

- `LocalSource` reads through `darwin-proc` in the current process. Running as
  root, it returns everything that root can read.
- `HelperSource` sends the request to the helper daemon.
- `FixtureSource` returns fixed records for tests.

The binaries choose a source once at start-up. They use `LocalSource` when the
effective user is root, `HelperSource` when the helper's socket exists and
passes the peer check, and `LocalSource` otherwise. An environment variable
forces a particular source for debugging and tests.

### Helper daemon

`procps-helperd` runs as root under launchd with socket activation. The plist's
`Sockets` entry creates a Unix stream socket in a root-owned directory under
`/var/run`, and the daemon obtains it with `launch_activate_socket`. launchd
starts the daemon on the first connection, and the daemon exits after a period
with no connections.

- The daemon only reads. It never sends signals: `pkill` calls `kill(2)` as the
  invoking user, so the kernel's permission checks still apply.
- It identifies each client with `getpeereid` on the connection.
- The client calls `getpeereid` on its end too and refuses to use a socket whose
  peer is not root.
- Requests and responses are versioned, length-prefixed and encoded with `serde`
  and `postcard`. The daemon rejects requests above a fixed size and stops work
  that exceeds a time limit.
- Each record contains the process start time. A client compares it with its own
  `kinfo_proc` data to detect a recycled pid.

The access policy follows Linux procfs defaults (no `hidepid`):

- Any user may read identity, command line, state, CPU times, memory sizes and
  page faults, thread lists, scheduling parameters, signal masks, and the text,
  data and shared sizes. Linux exposes these through `/proc/<pid>/stat`,
  `status`, `cmdline` and `statm`, which every user can read.
- Only root, or a caller whose user ID equals the target's real, effective and
  saved user IDs while the target has not changed credentials since its last
  `exec` (`PROC_FLAG_PSUGID` unset), may read the environment, working
  directory, file descriptors, disk I/O byte counts, and the unique,
  proportional and swapped sizes. Linux guards the equivalent files (`environ`,
  `cwd`, `fd`, `io`, `smaps`) with a ptrace read-access check, which this rule
  models.

Task 3.2 decides where each Darwin extension field falls.

### Other implementation choices

- Pattern matching in `pgrep`, `pkill` and `ps -C` uses the system's `regcomp`
  and `regexec` with `REG_EXTENDED`, as procps-ng does. The Rust `regex` crate's
  syntax differs from POSIX extended regular expressions, for example in bracket
  expressions and back-references.
- `top` draws the screen itself on top of `crossterm` for raw mode, the
  alternate screen and resize events. procps-ng's layout rules are ported
  directly, and a widget library would add a layout model that has to be worked
  around.
- `pgrep`, `pkill` and `procps-helperd` parse arguments with `clap`. procps-ng
  parses `pgrep` and `pkill` with `getopt_long`, whose syntax `clap` accepts.
  `pkill` first removes the first argument of the form `-<signal>`, as
  procps-ng's `signal_option` does. `clap` errors are rendered in procps-ng's
  format with exit status 2.
- `ps` and `top` use parsers ported from procps-ng. `ps` mixes BSD, UNIX and GNU
  forms in one command line, which `clap` cannot express. uutils/procps uses
  `clap` for `ps` and rejects `ps aux`, `ps -u USER`, `ps -s SID` and `--sort`
  as a result. Its tables of format specifiers and headers can still be reused.
- Errors use `thiserror` in every crate. Each binary's top-level error enum
  decides its exit status.
- The tools write through a locked, buffered standard output and restore the
  default `SIGPIPE` disposition, so a closed pipe terminates them silently as it
  terminates procps-ng.
- Library crates are instrumented with `tracing`. The tools install a subscriber
  only when `PROCPS_DARWIN_LOG` is set, so by default they print nothing beyond
  procps-ng's output.

`AGENTS.md` lists the coding conventions that follow from these choices.

## Tasks

Phases 0 to 2 come first. After that, phase 3 (helper) and phase 4 (`pgrep` and
`pkill`) can proceed in parallel; both are needed before phases 5 and 6 are
complete, but `ps` and `top` can start against `LocalSource`.

Each task follows red/green TDD where the change allows it, and is done only
when `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` pass.

### Phase 0: groundwork

#### 0.1 Data-availability spike (done)

`docs/darwin-data.md` records the results: group lists, pending and blocked
signals, `processor_set_statistics`, `PROC_PIDTHREADID64INFO` and the region
walk, each as an unprivileged user and as root.

#### 0.2 Repository scaffolding (done)

The Cargo workspace has the four crates, with strict lints in
`[workspace.lints]`: `gifopt`'s clippy settings, plus `allow_attributes`,
`allow_attributes_without_reason`, `undocumented_unsafe_blocks`, `missing_docs`,
`unsafe_code` and `unsafe_op_in_unsafe_fn`. `clippy.toml` makes the `cargo`
lints check the unpublished crates. The flake builds the workspace with crane
and checks it with clippy, the tests, the documentation build, `cargo deny`,
`cargo audit`, treefmt and `reuse lint`. The justfile has `build`, `clippy`,
`test`, `doc`, `deny`, `audit`, `reuse`, `fmt` and `check` recipes. `cargo-deny`
allows only licences that a dependency needs and that are compatible with
GPL-3.0-or-later. CI runs the flake checks on macOS.

#### 0.3 procps-ng reference harness (done)

`crates/procps-harness` runs reference scenarios. A scenario in
`scenarios/<tool>/<name>.toml` declares fixture processes with known names and
one command. The harness records the exit status, standard output and standard
error, and whether each fixture is still running, was killed or was stopped, so
`pkill` scenarios can check which processes received a signal. Each pid in the
output is replaced by its name: a fixture's name, `{session}` or `{command}`.

Each run starts a session leader with `setsid`, and the leader starts the
fixtures and then the command, so all of them are in one new session. The
command selects only processes in that session, for example with `pgrep -s 0`.
The runner also starts a decoy for each fixture, with the same name, outside the
session. A command that signals a decoy fails the run, and generation runs each
scenario with and without decoys and rejects it if the outcomes differ. Runs are
therefore independent, and the tests run them in parallel. When the leader
exits, or a timeout passes, the runner kills every process left in the session.

Output that depends on processes outside the session, such as `ps -e`, bare `ps`
and the summary area of `top`, cannot be checked this way; it belongs in
`FixtureSource` tests, or in a later harness feature that keeps only the
session's rows.

Masking replaces a pid with a name but leaves the spaces before it, so the
harness gives every masked process a five-digit pid. It starts processes until
the pid counter reaches 10000, and restarts a run whose pids fall outside 10000
to 99999 or are out of order.

`just harness-generate` runs every scenario against procps-ng 4.0.7 from
nixpkgs, built with procps-ng's default `top`, in a `nixos/nix` container. The
container presents a `pid_max` of 100000, which matches Darwin's pid range, and
runs each scenario's processes as `nobody`. It writes
`golden/<tool>/<name>.toml` and removes golden files that no scenario has. The
`reference` test in the `procps` package runs the same scenarios against this
project's binaries, one test per scenario, and compares the results with the
golden files. A scenario marked `pending` is expected to differ until its tool
is implemented. Its test fails when the scenario matches, so that the mark is
removed.

The golden files are TOML, which crane's `cleanCargoSource` keeps, so the flake
checks run the comparison too. The first two scenarios, `pgrep` by name and
`ps -o pid=,comm=`, are pending until phases 4 and 5. User and group names, and
uids, are not masked yet; the first `ps` scenario with user columns needs that.

### Phase 1: Darwin data layer (`darwin-proc`)

#### 1.1 FFI additions (done with 1.2)

The FFI declarations listed under Architecture are in `ffi.rs`, with tests that
compare their sizes and field offsets with the SDK. Each task adds the
declarations that it uses, so every declaration has a caller. The workspace
denies `unsafe_code`, and `darwin-proc` opts out at its root.

#### 1.2 Process enumeration and identity (done)

`ProcessInfo` decodes `kinfo_proc`, read for every process with one
`kern.proc.all` sysctl or for one process with `kern.proc.pid`. `Pid` has
`executable_path` (`proc_pidpath`) and `session` (`getsid`). Errors distinguish
an exited process, a denied read, and data that the kernel does not provide for
a live process, such as `kernel_task`'s executable path.

#### 1.3 Arguments and environment (done)

`Pid::arguments` parses `KERN_PROCARGS2` into the executable path, argv and
environment. Unit tests use byte buffers in the layouts that macOS 27 returns,
including truncated buffers and buffers from processes that have overwritten
their arguments. When another process reads the arguments of an Apple platform
binary such as `/bin/cat`, macOS 27 returns its arguments but not its
environment, even if both processes belong to the same user. Other processes may
be treated the same way. If such a process has an empty `argv[0]`, the kernel
also returns its first environment string. macOS returns the whole environment
of an ad hoc signed program to any process of the same user. macOS 26, which CI
uses, returns the environment of platform binaries too.

#### 1.4 CPU, memory and threads (done)

`Pid::task_info` reads `PROC_PIDTASKINFO`, `Pid::resource_usage` reads
`proc_pid_rusage` with `RUSAGE_INFO_V6`, and `Pid::threads` reads each thread
with `PROC_PIDTHREADID64INFO`, using the IDs that the private
`PROC_PIDLISTTHREADIDS` lists. `PROC_PIDTASKINFO` and `proc_pid_rusage` report
CPU time in Mach absolute time, whose unit is 125/3 nanoseconds on Apple
silicon, while `PROC_PIDTHREADID64INFO` reports nanoseconds. macOS reports 0 for
every thread's sleep time.

#### 1.5 Memory region walk (done)

`Pid::regions` calls `proc_pidinfo` with `PROC_PIDREGIONINFO` repeatedly, from
address 0. It returns each region's address and size, protection, share mode,
user tag, memory object ID, reference count and flags, and its resident, swapped
and dirtied sizes in bytes. Task 2.3 decides which of the sizes that `ps` and
`top` show come from the regions.

The kernel counts the private and shared resident pages in its own 16 KiB pages.
It reports the other page counts in the smaller of the caller's and the target's
page sizes. No call reports another process's page size, so `darwin-proc`
converts every page count with the caller's. On Apple silicon every arm64
process has 16 KiB pages. Only an x86_64 program running under Rosetta can have
4 KiB pages, and its `resident`, `shared_now_private`, `swapped_out` and
`dirtied` sizes are then four times too large.

For each region that has a memory object, the kernel visits every page and
checks whether it is resident. A resident page costs much more to visit than a
page that is not resident, so a walk takes time mostly in proportion to the
process's resident memory. A walk of every process that an unprivileged user can
read, 817 processes with 1.28 million regions in total, took 3.7 seconds. A walk
of an OrbStack helper alone, with 4,831 regions and 1.6 GiB resident, took 860
ms. A walk of a Chrome renderer with 199,201 regions and 330 MiB resident took
130 ms.

#### 1.6 Other per-process data (done)

`Pid::working_directory` reads `PROC_PIDVNODEPATHINFO`, and
`Pid::file_descriptors` lists the open descriptors with `PROC_PIDLISTFDS`. Both
calls fail with `EPERM` for another user's process and for `kernel_task`, so the
helper has to read the working directory and descriptors of other users'
processes. `Terminal::name` finds a terminal's name in `/dev` with `devname_r`.
`Uid` and `Gid` look up names from IDs and IDs from names with `getpwuid_r`,
`getgrgid_r`, `getpwnam_r` and `getgrnam_r`. Each lookup is an IPC call to
`opendirectoryd`, so `procps-core` should cache the names.

#### 1.7 System statistics (done)

`Host` reads the load averages with `getloadavg`, the boot time from
`kern.boottime`, the memory from `hw.memsize` and `host_statistics64`, the swap
space from `vm.swapusage`, each processor's ticks with `host_processor_info`,
the task and thread totals with `processor_set_statistics`, and the number of
login sessions from the login records. Any user can read all of them. The Mach
calls return a `kern_return_t`, which `Error::Mach` reports.

### Phase 2: core model (`procps-core`)

#### 2.1 Domain model (done)

`procps-core` has `Field<T>`, the `FieldGroup`s, `SnapshotRequest`, and the
`Snapshot`, `Process` and `System` records. The records contain the
`darwin-proc` types, so the model does not repeat their fields. A value of
`None` in a record means that the request did not ask for its group.

CPU time and memory form one `Usage` group, because both come from
`PROC_PIDTASKINFO` and `proc_pid_rusage`, which need the same permission. For a
zombie only `proc_pid_rusage` succeeds, and procps-ng shows a zombie's CPU time,
so the group keeps a separate `Field` for each call. To let `top` measure the
interval between two snapshots, a snapshot also records the time since boot from
`mach_continuous_time`. Unlike the wall clock, which can be set backwards, the
time since boot never decreases.

#### 2.2 Sources (done)

`ProcessSource`, `LocalSource` and `FixtureSource`. `LocalSource` leaves out a
process that exits while `LocalSource` reads it, and turns every other
`darwin-proc` error into a `Field`: denied, unsupported, or failed with a logged
warning. It reads `KERN_PROCARGS2` once when a request asks for both arguments
and environment. macOS 27 withholds the environment of a platform binary from
other processes, and returns no strings in its place, or only the first string
if the binary's `argv[0]` is empty. `LocalSource` therefore reports another
process's environment as denied when the environment has no strings, or has one
string and the process's `argv[0]` is empty, even if that is the real
environment. A `FixtureSource` reports a requested group that its fixture lacks
as unsupported.

#### 2.3 Linux mappings (done)

Implement and document in `docs/mappings.md`:

- State letters and modifiers (done). With neither the threads nor the task
  information, the state letter is `-`.
- Priority and nice scales for `pri`, `PR` and `NI` (done).
- Scheduling policy names (done).
- Bits of the `f` column, where a Darwin flag has the same meaning (done).
- Memory summary figures (done). `buff/cache` is file-backed pages plus
  purgeable pages, which matches Activity Monitor's "Cached Files", and `avail`
  is free, file-backed and purgeable pages together. XNU counts the speculative
  pages as file-backed too, so they are not added again. procps-ng computes
  `used` as the total minus the available memory, so `free`, `used` and
  `buff/cache` do not add up to the total.
- CPU states (done): Darwin counts all user-mode time as user time and reports
  no nice ticks, so `ni` shows `0.0` and `us` includes the time of niced
  processes.
- The sizes for `trs`, `drs`, `size` and `sz` (done). procps-ng computes `trs`
  and `drs` from the virtual size and the bounds of the code segment in
  `/proc/<pid>/stat`, not from the memory regions. `size` is `VmData` plus
  `VmStk` from `/proc/<pid>/status`, in KiB, and `sz` is the virtual size in
  pages. Darwin does not report the code segment's bounds, so `trs` counts the
  executable regions that are not submaps, which leaves out the shared cache,
  and `size` counts the private writable regions that are not submaps.
- Proportional set size (done): Darwin does not report how many processes share
  each page, so `pss` shows `-`. `uss` is the private resident memory.
- The command-name length (done): the tools cut `p_comm` to Linux's 15 bytes, so
  `pgrep` matches and warns as on Linux.
- How `args` is shown when the arguments are denied (done). procps-ng shows
  `[comm]` when it cannot read `/proc/<pid>/cmdline`, for any reason, so a
  denied command line shows `[comm]` as on Linux.
- What `kernel_task` shows in the `exe` column and the size columns (done): `-`
  for the executable and the region columns, and `[kernel_task]` for its command
  line.

### Phase 3: privileged helper

#### 3.1 Protocol (done)

Request and response types, a version check, framing and size limits, with
round-trip tests. `darwin-proc` derives `serde`'s traits behind its `serde`
feature, which `procps-core` enables, so `procps-core`'s records need no
separate wire types. Paths use `serde`'s encoding of an `OsString`, because
`serde` cannot serialise a `PathBuf` that is not valid UTF-8.

A message is a big-endian `u16` protocol version, a big-endian `u32` length and
a body encoded with `postcard`. The tools and the helper are built together, so
the protocol has one version and no negotiation. A receiver checks the version
before it decodes the body, because `postcard`'s encoding does not describe
itself. `VERSION` must increase whenever the encoding changes. A test compares
fixed messages, which use every enum variant, with their version 1 encoding. It
fails when a field or variant is added, removed or moved, but not when an enum
variant is added at the end. When the versions differ, the helper replies with a
refusal whose header has the helper's version. A request body can have at most 1
MiB, and a response body at most 256 MiB. The receiver checks the length before
it reads the body, and the body's buffer grows only as the bytes arrive.

#### 3.2 Access policy

The rules under Architecture, including a decision for each Darwin extension
field. Tests inject caller credentials and a `FixtureSource`. The `Usage` group
includes the numbers of bytes that a process has read from and written to disk.
Linux lets only a process's owner read those counts in `/proc/<pid>/io`. The
policy can let any user read the CPU time and memory in the group, so it has to
withhold the disk counts separately.

#### 3.3 Daemon

Socket activation, `getpeereid`, request handling through `LocalSource`, time
limits, idle exit and logging. An integration test runs the daemon unprivileged
on a temporary socket to exercise the transport. The time limit covers the whole
connection, so a tool that sends its request one byte at a time cannot keep the
connection open beyond the time limit. The daemon replies with a `Refusal` when
it cannot take the snapshot, runs out of time, or would send a response larger
than the limit.

#### 3.4 Client

`HelperSource`, source selection at start-up, the root peer check and the
override variable. A tool reads process data itself after any refusal or
protocol error. When a request has another version or is too large, the helper
sends the refusal and closes the connection without reading the rest of the
request, so a tool that is still writing gets `BrokenPipe` from the write before
it reads the refusal.

#### 3.5 Packaging

The launchd plist, a nix-darwin module that installs the daemon and its socket,
and install and uninstall instructions for users without nix-darwin.

#### 3.6 Threat model

`docs/helper-security.md` covering untrusted local clients, denial of service
through expensive requests, information disclosure under the access policy, pid
reuse, and spoofed sockets.

### Phase 4: pgrep and pkill

#### 4.1 Option parsing

Parse every procps-ng 4.0.7 option, including `-<signal>`, `--signal`, combined
short options and long options. Reference scenarios cover usage text, error
messages and exit codes (0 for a match, 1 for no match, 2 for a syntax error, 3
for a fatal error).

#### 4.2 Matching

Match by pattern, `-f`, `-i`, `-x`, `-v`, `-u`, `-U`, `-G`, `-g`, `-P`, `-s`,
`-t`, `-p`, `-F`, `-L`, `-r`, `-H` (from the caught-signal mask), `-O`, `--env`,
`-n`, `-o` and `-A`, excluding the tool's own process.

#### 4.3 Output

Implement the output options `-l`, `-a`, `-d`, `-c`, `-Q`, `--quiet`, and `-e`
in `pkill`. `-w` lists 64-bit Darwin thread IDs in place of Linux TIDs; document
this in `docs/mappings.md`.

#### 4.4 Signals

Parse signal names and numbers for Darwin's signal set (which has `SIGINFO` and
`SIGEMT` and no real-time signals) and send them with `kill(2)`. Report failures
in procps-ng's format.

#### 4.5 Unsupported options

`-m`, `-q`, `--cgroup`, `--ns` and `--nslist` exit with status 2 as described
under compatibility rules.

#### 4.6 Tests

Reference scenarios through the harness, and macOS integration tests that start
children with known arguments, environment, nice value, process group, session
and pseudo-terminal.

### Phase 5: ps

#### 5.1 Option parsing

Port procps-ng's parser, including the UNIX, BSD and GNU syntaxes, the rules for
mixing them, and the warning for `ps -aux`. Support the default personality
first and the other `PS_PERSONALITY` values after the rest of phase 5.

#### 5.2 Process selection

Select processes with `-A`, `-e`, `-a`, `-d`, `-N`, `T`, `r`, `x`, `-C`, `-G`,
`-g`, `-p`, `-q`, `-s`, `-t`, `-u`, `-U`, `--ppid` and the other long forms.

#### 5.3 Format engine

Format lists, custom headers (`=`), widths (`:`), sorting (`--sort`, `k`),
`PS_FORMAT`, and the predefined formats (`-f`, `-F`, `-l`, `-j`, `j`, `l`, `s`,
`u`, `v`, `X`, `-y`).

#### 5.4 Format specifiers

All 153 specifiers, grouped by data source as in the appendix, plus the Darwin
extensions. Candidate extensions: physical footprint, peak footprint, energy,
instructions, cycles, wired memory, context switches, CPU architecture, and
whether the process runs under Rosetta. Final names are chosen under
compatibility rule 5.

#### 5.5 Threads and forest

`-L`, `-T`, `H`, `m`, `-m`, `--forest`, `-H` and `f`.

#### 5.6 Output modifiers

`c`, `e`, `n`, `S`, `h`, `--headers`, `--no-headers`, `w`, `ww`, `--cols`, and
the `COLUMNS` and `LINES` environment variables.

#### 5.7 Tests

Reference scenarios for each predefined format and modifier, integration tests,
and a check that numeric columns roughly agree with `/bin/ps` for the same
processes.

### Phase 6: top

#### 6.1 Sampling

Periodic snapshots and the per-interval deltas for `%CPU`, page-fault deltas and
I/O, with thread mode and the Irix and Solaris CPU modes.

#### 6.2 Summary area

Uptime and load, task and thread counts by state, CPU lines (`us`, `sy`, `ni`
and `id` from Darwin ticks; `wa`, `hi`, `si` and `st` show `0.0`), per-CPU
lines, the memory and swap lines using the 2.3 mapping, and the bar and block
graph modes.

procps-ng 4.0.7's `top` can show only the performance cores or only the
efficiency cores. Apple silicon has both kinds, so that toggle needs each
processor's core type, which `Host::processors` does not report yet.

#### 6.3 Fields and sorting

Every procps-ng field with its Darwin source, `-` for concepts that macOS lacks,
and the Darwin extensions from 5.4.

#### 6.4 Batch mode

`-b` with all command-line options (`-n`, `-d`, `-p`, `-u`, `-U`, `-o`, `-O`,
`-w`, `-H`, `-i`, `-c`, `-S`, `-E`, `-e`, `-1`, `-s`, `-A`), verified through
the reference harness. Batch mode comes before interactive mode because scripts
depend on its exact output.

#### 6.5 Interactive mode

Terminal setup and resize handling, then the command set in groups: help and
quit; sorting and field management (`f`, `<`, `>`); filtering (`o`, `O`, `u`,
`U`, `i`); display toggles (`c`, `H`, `V`, `x`, `y`, `z`, `1`, `t`, `m`); colour
(`Z`); alternate windows (`A`, `g`, `a`, `w`); signalling and renicing (`k`,
`r`); locate (`L`, `&`); and inspect (`Y`).

#### 6.6 Configuration

Read and write `toprc` in procps-ng 4's format (`$XDG_CONFIG_HOME/procps/toprc`,
falling back to `~/.toprc`), read `/etc/topdefaultrc`, and save with `W`. A
`toprc` from a Linux machine should load without errors.

#### 6.7 Secure mode

`-s` and the system-wide secure-mode configuration.

### Phase 7: packaging and documentation

#### 7.1 Installation

Flake package and overlay; nix-darwin module that installs the binaries and,
optionally, the helper.

#### 7.2 Man pages

Adapt procps-ng's man pages, and give each a section on differences from Linux.

#### 7.3 README

Write a README covering installation, the helper and its access policy, and a
summary of `docs/mappings.md`.

## Testing

- Unit tests run against `FixtureSource`, so selection, sorting and formatting
  are tested without depending on the live process table. Assertions compare
  whole records or whole output strings with `pretty_assertions`, similar cases
  use `rstest`, and error variants are checked with `assert_matches`.
- The reference harness (task 0.3) is the oracle for option parsing, error
  messages, exit codes and output layout.
- macOS integration tests start child processes with controlled attributes and
  inspect them with the real binaries. Tests that need root run under `sudo` in
  CI; GitHub's macOS runners allow passwordless `sudo`. These tests need the
  live process table, so CI runs them in a step of their own, outside
  `nix flake check`. The reference test also reads the live process table, but
  each scenario selects only processes in its own session, so it runs inside
  `nix flake check`.
- Helper tests cover the protocol, the access policy with injected credentials,
  and the transport on a temporary socket.

## Risks and open questions

- Root's access to process data comes from current kernel behaviour, not a
  documented guarantee. A future macOS could restrict `proc_pidinfo` for
  platform binaries, as it already restricts `task_for_pid`. Those columns would
  then show `-` even with the helper.
- CI needs macOS 27 runners. Until they exist, CI runs on the newest available
  runner and some tests may need to be skipped there.
- The interactive part of `top` is the largest single item. If it needs cutting,
  inspect mode (`Y`) and alternate windows (`A`) are the least used parts.
- Walking memory regions for every process takes several seconds, which is
  longer than `top`'s default refresh interval of 3 seconds (task 1.5). `top`
  should walk regions only when a displayed field needs them, and those fields
  may need a slower refresh or a cache.
- A pid can be reused between enumeration and a later read. Records compare
  start times to detect this; the tools then treat the process as exited.

## Appendix: ps format specifiers by data source

"Any user" means the value is readable for every process without the helper.
"Helper" means it needs the helper, root, or the caller's own process. "Helper,
owner" means the helper returns it only under the restricted part of the access
policy. Task 0.1 may move individual specifiers between groups.

| Data source                                                      | Specifiers                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Any user: `kinfo_proc`, `proc_pidpath`, `getsid`                 | `pid`, `tgid`, `ppid`, `pgid`, `pgrp`, `sid`, `sess`, `tpgid`, `tty`, `tt`, `tname`, `uid`, `euid`, `ruid`, `suid`, `svuid`, `user`, `euser`, `ruser`, `suser`, `uname`, `gid`, `egid`, `rgid`, `sgid`, `svgid`, `group`, `egroup`, `rgroup`, `sgroup`, `supgid`, `supgrp`, `ni`, `nice`, `start`, `stime`, `lstart`, `bsdstart`, `start_time`, `etime`, `etimes`, `comm`, `ucmd`, `ucomm`, `fname`, `exe`, `caught`, `sigcatch`, `ignored`, `sigignore`, `pending`, `sig`, `f`, `flag`, `flags` |
| Any user, approximated: effective IDs in place of filesystem IDs | `fuid`, `fuser`, `fgid`, `fgroup`                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| Helper: task info, rusage, threads, arguments                    | `args`, `cmd`, `command`, `%cpu`, `pcpu`, `c`, `cp`, `cuc`, `cuu`, `time`, `cputime`, `cputimes`, `times`, `bsdtime`, `%mem`, `pmem`, `rss`, `rssize`, `rsz`, `vsz`, `vsize`, `maj_flt`, `min_flt`, `nlwp`, `thcount`, `lwp`, `tid`, `spid`, `pri`, `rtprio`, `policy`, `class`, `cls`, `sched`, `blocked`, `sigmask`, and the full `stat`, `s` and `state`                                                                                                                                      |
| Helper, owner                                                    | `environ`, `fds`, `rbytes`, `wbytes`                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Helper: memory region walk                                       | `drs`, `trs`, `size`, `sz`                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Helper, owner: memory region walk                                | `uss`, `pss` (if approximated)                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Not applicable: shows `-`                                        | `ag_id`, `ag_nice`, `cgname`, `cgroup`, `cgroupns`, `docker`, `ipcns`, `label`, `lsession`, `luid`, `lxc`, `machine`, `mntns`, `netns`, `numa`, `oom`, `oomadj`, `ouid`, `pcap`, `pcaps`, `pidns`, `seat`, `slice`, `timens`, `unit`, `userns`, `utsns`, `uunit`, `htprv`, `htshr`, `eip`, `esp`, `stackp`, `psr`, `cpu`, `cpuid`, `lastcpu`, `sgi_p`, `wchan`, `nwchan`, `rchars`, `wchars`, `rops`, `wops`, `wcbytes`                                                                          |
