<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# The helper's threat model

`procps-helperd` runs as root and serves every local user, so a mistake in it
can disclose another user's process data, or make the tools slow or show less
for everyone. This file describes what the helper defends against, how, and
which limits remain. [`helper.md`][helper] describes how the helper runs and how
to install it.

The tools are not finished yet. Where this file describes what a tool does, it
describes the client in `procps-core`, which the tools will use to read process
data.

[helper]: helper.md

## What the helper trusts

The helper trusts the kernel and launchd. It does not trust its clients: any
local user can connect to its socket and send it any bytes.

The tools trust the socket at `/var/run/procps-helperd.sock` only if it is a
socket that root owns with no other links, and `getpeereid` reports that root
called `listen` on it. Only root can create a socket that root owns, either
itself or through launchd. When the helper's own socket is in place, the process
that answers is the helper, which runs as root. A root process can already read
and signal every process, so lying to the tools would give it no new capability.
A process in the `daemon` group can put another daemon's socket in place, and
that daemon may run as another user. [Spoofed sockets][spoofed] describes this
case and the other cases that these checks leave open.

[spoofed]: #spoofed-sockets

Remote attackers are out of scope. The helper listens only on a Unix socket. A
request contains only pids, groups of values and a flag for the system
statistics, and the helper never reads a path or a command from a client.

## Untrusted local clients

A client sends one request: the processes to read, the groups of values to read
for them, and whether to read the system statistics. The helper reads process
data and returns it. It never signals a process or writes to one. `pkill` will
signal processes with `kill(2)` as the user who runs it, so the kernel's
permission checks will apply to every signal.

The helper reads a request as a frame with a 2-byte version and a 4-byte length,
then decodes the body with `postcard`. It refuses a frame with another version
or a body longer than 1 MiB without reading the rest, and closes the connection.
It also refuses a body that does not decode to exactly one request. Each
connection has its own thread, which closes the connection when it has sent one
response. A client that disconnects while the helper reads its snapshot does not
free the worker until the snapshot is finished.

The helper identifies the client with `getpeereid`, which returns the effective
user and group IDs that the client had when it connected. A process that hands
its connection to another process therefore passes on its own identity. A
set-user-ID-root program that connects to the helper and then passes the
connection to an unprivileged process would give that process root's view, as it
would by passing on any other descriptor that it opened as root.

## Information disclosure under the access policy

The helper applies the access rules of Linux's `/proc` without `hidepid`, in
place of the rules that macOS applies.

- Every user can read every process's identity, command line, state, CPU times,
  memory sizes, faults, threads and their names, scheduling data, signal masks,
  system call and Mach message counts, wakeups, and the code and data sizes that
  the helper totals from the process's memory regions. macOS withholds all of
  these from other users except the identity, state, scheduling data and signal
  masks, so installing the helper discloses the rest, as Linux does. A password
  passed as a command-line argument becomes visible to every local user.
- A process's private values are its environment, working directory, file
  descriptors, memory regions, disk I/O counts, and instruction, cycle and
  energy counters. Root can read them. Another user can read them only if their
  user ID equals all three of the process's user IDs, their group ID equals all
  three of its group IDs, and the process does not have `P_SUGID` set. XNU sets
  `P_SUGID` when a process changes its credentials or executes a set-user-ID or
  set-group-ID program. Linux marks such a process as not dumpable, and Linux's
  ptrace read-access check then refuses every caller except root, including the
  process's owner.

The helper reads the private values of every process in the request, then
replaces each value that the client may not read with a marker that the tools
show as `-`. The withheld values never leave the helper. The time that the
helper takes to answer still depends on them, so a client can estimate, for
example, how many file descriptors another user's process has open.

The helper has no equivalent of `hidepid`. An administrator who wants other
users' processes hidden should not install the helper.

## Processes that change their credentials

The helper reads a process's credentials before its private values, and the
process can execute a set-user-ID program in between. After it has read the
snapshot, the helper therefore reads the credentials of each process again, and
passes on the private values only if the client may read the process both times.

One sequence passes both checks. Between the helper's two readings, the process
executes a set-user-ID program, the program resets all its user and group IDs to
those of the user who ran it, and then executes another program, which clears
`P_SUGID`. The process then looks unchanged, although the helper read some of
its private values while it ran the set-user-ID program. The client receives
those private values. The file descriptors and memory regions can include files
that the set-user-ID program opened with its privileges. The window is the time
that the helper takes to read the snapshot.

## Pid reuse

Each record contains the process start time. When it reads the credentials
again, the helper compares the start times, and withholds the private values of
a process whose pid now belongs to a process that started later.

The record that a tool receives can still be out of date. `pkill` will read each
process's start time again just before `kill(2)` and skip a process whose start
time has changed. A process can still exit and have its pid reused between that
check and `kill(2)`.

## Denial of service

A local user can make the helper slow or unavailable for other users. The helper
limits each user's connections, and gives priority to waiting requests from
users with fewer snapshots running. A tool that the helper refuses reads process
data itself and shows what macOS gives its user. Other users' command lines, for
example, show as `[comm]`, and `pgrep -f` and `pkill -f` cannot match them. What
these commands match for other users can therefore change from one run to the
next.

### Connections

The helper keeps at most 256 connections open, 64 of them for one user, and
refuses a connection beyond those limits with `Busy`. The limits protect the
helper's descriptors and threads. A client has 1 second to send its request, and
each read and write uses the time that remains, so a client that sends one byte
at a time keeps a connection for at most 1 second.

### Expensive requests

The helper has one worker for each processor, and a worker takes one snapshot at
a time. A walk of a process's memory regions visits each region, and a process
can split its memory into millions of regions. When a request asks for the code
and data totals, which every user receives, the helper walks the regions of
every process that the request selects, whoever sent the request. Any user
therefore controls the cost of every such snapshot that includes their
processes, including other users' snapshots.

When every worker is busy, a free worker goes to the waiting request whose user
has the fewest snapshots running. A user who sends many expensive requests
therefore delays mostly their own, but can still keep every processor busy. A
user whose processes have many regions slows other users' snapshots without
sending a request, and the priority order charges that cost to the other users.
The helper does not limit the processor time that one user's requests use.

The helper has 10 seconds, including the wait for a worker, to read the process
data for a request. It refuses a request with `Busy` when its wait for a worker
reaches that limit, or when a worker becomes free with less than half of the
limit left. A user who keeps every worker busy can therefore make the helper
refuse other users. The helper cannot interrupt a snapshot, so a slow snapshot
keeps its worker beyond the limit, and the helper then refuses it with
`TimedOut`.

### Memory

A response can be up to 256 MiB long, but the helper builds the snapshot and its
encoding before it checks that limit. A user who controls a snapshot's size can
therefore make the helper keep more than 256 MiB in memory for each connection.
Each of up to 256 connections can keep a snapshot and its encoding until the
client has received the response or the response's 10 seconds have passed. The
helper's resident memory can stay high after those connections have closed,
until the helper exits.

### A stuck helper

A watchdog stops the helper when a connection runs 30 seconds beyond its time
limits, 51 seconds in all by default, because a read from the kernel can block
and the helper cannot interrupt it. launchd starts a new helper for the next
connection, but the connections that were open fail. A user who can make a
snapshot take that long can trigger the watchdog on purpose, and repeat it.

A timeout or `Busy` does not make a tool stop using the helper. While
connections get stuck, each of `top`'s refreshes can therefore wait up to 30
seconds before `top` reads process data itself.

Task 3.7 in `PLAN.md` is to make a snapshot stop at its time and size limits.
That would remove the costs that a user controls from [Expensive
requests][expensive], [Memory][memory] and this section. A read from the kernel
that blocks would still trigger the watchdog.

[expensive]: #expensive-requests
[memory]: #memory

### Logs

At the default level, the helper logs its start and stop, snapshots that exceed
a limit, and errors that a client cannot cause. Any user can send requests as
fast as the helper answers them, so the helper logs each request, and each value
that it cannot read from a process, only at the `debug` level, which is off by
default. A snapshot that exceeds a limit keeps a worker for seconds, which
limits how often one user can make the helper log a warning.

The helper receives its log as standard error from launchd and cannot reopen it.
newsyslog rotates the log by renaming it, and the helper writes to the rotated
file until it exits. A helper that has run for an hour exits at the next check
that finds no open connection, so a user who keeps a connection open at every
check keeps the helper writing to the rotated file, and newsyslog cannot limit
that file's size.

## Spoofed sockets

launchd creates the socket at `/var/run/procps-helperd.sock` as root, with
mode 0666. `/var/run` belongs to root and the `daemon` group, with mode 0775 and
no sticky bit. A process that runs as a member of the `daemon` group can
therefore remove or replace the socket at any time.

Before it connects, a tool checks that the path is a socket that root owns with
no other links, and reads process data itself otherwise. A symbolic link fails
the check because `lstat` reports a link. launchd creates a user's launch agent
socket owned by that user, so another user cannot make launchd create a socket
that root owns. A process in the `daemon` group can hard-link a socket that root
owns, such as `/var/run/mDNSResponder`, to the helper's path, and the link count
catches that link. After it connects, the tool also checks that `getpeereid`
reports root, which means that root called `listen` on the socket. launchd calls
`listen` as root for every job's socket, including a user's own launch agent, so
this second check catches only a listener that launchd did not create. A tool
treats a failure of either check as lasting, and does not ask the helper again
in that run.

A process in the `daemon` group can still defeat the checks in two ways, with
different results.

- It can replace the path between a tool's check and its connection, for example
  with a symbolic link to its own launch agent's socket. The tool then connects
  to a socket that the process controls. The process can return false data to
  the tool, and learns with `getpeereid` which user connected and when.
- It can move another system daemon's socket, which root owns, to the helper's
  path. That socket passes both checks. A daemon that accepts the connection and
  never replies makes each request wait for the tool's 30-second deadline. A
  timeout does not make a tool stop using the helper, so `top` waits on every
  refresh. The process learns which users connect, and can return false data,
  only if it also controls that daemon.

Only root and processes in the `daemon` group can do either.

## The log file

Each event that a client causes includes the client's user ID, group ID and pid.
The user and group IDs come from `getpeereid` and are reliable. The pid comes
from `LOCAL_PEERPID`, which reports the last process to use the client's end of
the connection, so a client can make the log show another process of its own.
The log can therefore record which users ran the tools and when: every request
at the `debug` level, and requests that exceed a limit at the default level. The
installations create `/var/log/procps-helperd.log` owned by root and the `admin`
group with mode 0640, and newsyslog creates each new log with the same mode. If
the file is deleted, launchd creates it again with mode 0644 when it next starts
the helper.
