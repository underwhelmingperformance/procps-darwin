<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# What Darwin returns to whom

These results come from probe programs run on macOS 27.0 (build 26A428) on Apple
silicon, once as an unprivileged user and once as root. The targets were the
probe itself, children that it started, the user's own shell, `launchd` (pid 1,
root), `WindowServer` (`_windowserver`), `mds` (root) and `kernel_task` (pid 0).
`PLAN.md` summarises the earlier probes under "What macOS exposes"; this file
records the checks that task 0.1 added.

| Call                                              | Caller's own process                                                                                                      | Another user's process | As root                                                          |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- | ---------------------- | ---------------------------------------------------------------- |
| `kinfo_proc` group list (`cr_groups`)             | Up to 16 groups                                                                                                           | Up to 16 groups        | Up to 16 groups; root's own list of 22 is cut to 16              |
| `kinfo_proc` pending and blocked signals          | Always 0, even with a signal blocked and pending                                                                          | Always 0               | Always 0                                                         |
| `kinfo_proc` ignored and caught signals           | Reported                                                                                                                  | Reported               | Reported                                                         |
| `processor_set_statistics` task and thread totals | Succeeds without privileges                                                                                               | Not applicable         | Succeeds                                                         |
| `PROC_PIDLISTTHREADIDS`, `PROC_PIDTHREADID64INFO` | Succeeds                                                                                                                  | Fails with `EPERM`     | Succeeds for every target, including `kernel_task`'s 845 threads |
| Region walk (`PROC_PIDREGIONINFO`)                | Succeeds                                                                                                                  | Fails with `EPERM`     | Succeeds, except for `kernel_task`, which fails with `EPERM`     |
| `KERN_PROCARGS2` arguments                        | Succeeds                                                                                                                  | Fails with `EINVAL`    | Succeeds, except for `kernel_task`, which fails with `EINVAL`    |
| `KERN_PROCARGS2` environment                      | Returned for the calling process itself and for ad hoc signed programs; withheld for platform binaries such as `/bin/zsh` | Not applicable         | Withheld for platform binaries, as for any other caller          |

## Consequences

- The `pending` and `blocked` columns cannot come from `kinfo_proc`, which
  reported 0 for every target that the probes tried. No other call that the
  probes tried returns them, so they show `-` (task 2.3).
- A group list from `kinfo_proc` stops at 16 groups. `supgid` and `supgrp` show
  at most 16 groups unless a later task finds a fuller source.
- `top`'s task and thread totals can come from `processor_set_statistics`
  without the helper.
- The helper can read threads for every target that the probes tried, and walk
  memory regions for every target except `kernel_task`.
- macOS 27 withholds the environment of Apple's platform binaries, and possibly
  of other processes, from every other process, including processes running as
  root. Neither the helper nor `ps e` run as root can show it. macOS 27 returns
  the environment of an ad hoc signed program to any process of the same user.
  On macOS 26.6.2 (build 25G83), another process of the same user could read the
  environment of the platform binary `/bin/cat`: a darwin-proc test did so on
  GitHub's `macos-26` runner, in run 37499052765 of the CI workflow.
