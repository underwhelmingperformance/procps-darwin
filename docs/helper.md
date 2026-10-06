<!--
SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>

SPDX-License-Identifier: GPL-3.0-or-later
-->

# The helper daemon

macOS lets a user other than root read the pids, user and group IDs, terminal,
command name, scheduling data and start time of other users' processes. It
withholds their CPU times, memory sizes, threads, arguments, environment,
working directory and open files.

`procps-helperd` runs as root, reads process data for the tools, and applies the
access rules of Linux's `/proc` without `hidepid` before it returns the data.
Every user can then read another user's command line, CPU times and memory
sizes. A process's environment, working directory and open files are readable by
root, and by a user who matches all of the process's user and group IDs if the
process has not changed its user or group IDs.

The tools are not finished yet. Once they are, a tool run by a user other than
root will read process data through the helper when the helper's socket exists,
and will read process data itself when the helper fails.

launchd starts the helper when a tool connects to
`/var/run/procps-helperd.sock`. The helper exits 60 seconds after it starts or
after its last connection closes, whichever is later. After it has run for an
hour, it also exits when a tool connects while it has no open connection, or
within a second once it has no open connection. launchd then starts a new helper
for the next connection.

The helper logs JSON to `/var/log/procps-helperd.log`, and newsyslog rotates the
log. The log records which users ran the tools and when, so only root and
members of the `admin` group can read it.

## Installing with nix-darwin

If the helper is installed without nix-darwin, remove that installation first,
as [described below][removing]. nix-darwin stops with an error when it finds the
newsyslog rule in `/etc` that it did not install.

[removing]: #removing-an-installation-without-nix-darwin

Add this repository's flake as an input, import its module and enable the
helper. The flake uses nix-darwin in its checks, so make its `nix-darwin` input
follow yours:

```nix
{
  inputs = {
    nix-darwin.url = "github:nix-darwin/nix-darwin";

    procps-darwin = {
      url = "github:underwhelmingperformance/procps-darwin";
      inputs.nix-darwin.follows = "nix-darwin";
    };
  };

  outputs = {nix-darwin, procps-darwin, ...}: {
    darwinConfigurations.example = nix-darwin.lib.darwinSystem {
      modules = [
        procps-darwin.darwinModules.default
        {
          nixpkgs.hostPlatform = "aarch64-darwin";
          services.procps-helperd.enable = true;
          system.stateVersion = 6;
        }
      ];
    };
  };
}
```

`darwin-rebuild switch` then installs the launchd job and the newsyslog rule.
Set `services.procps-helperd.enable = false` to remove them. nix-darwin leaves
the log files. To delete them, run `sudo rm -f /var/log/procps-helperd.log*`.

## Installing without nix-darwin

Build the helper, then install the binary, the log, the launchd job and the
newsyslog rule as root, and load the job:

```sh
cargo build --release --locked -p procps-helperd

sudo install -d -o root -g wheel -m 755 /usr/local/libexec
sudo install -o root -g wheel -m 755 target/release/procps-helperd \
  /usr/local/libexec/procps-helperd
sudo install -o root -g admin -m 640 /dev/null /var/log/procps-helperd.log
sudo install -o root -g wheel -m 644 \
  packaging/launchd/io.github.underwhelmingperformance.procps-helperd.plist \
  /Library/LaunchDaemons/
sudo install -o root -g wheel -m 644 packaging/newsyslog/procps-helperd.conf \
  /etc/newsyslog.d/
sudo launchctl bootstrap system \
  /Library/LaunchDaemons/io.github.underwhelmingperformance.procps-helperd.plist
```

The job runs the binary from `/usr/local/libexec/procps-helperd`. To install the
binary elsewhere, change the path in the property list too.

## Removing an installation without nix-darwin

Unload the job, then delete the files that the installation created:

```sh
sudo launchctl bootout system/io.github.underwhelmingperformance.procps-helperd
sudo rm -f \
  /Library/LaunchDaemons/io.github.underwhelmingperformance.procps-helperd.plist \
  /etc/newsyslog.d/procps-helperd.conf \
  /usr/local/libexec/procps-helperd \
  /var/run/procps-helperd.sock \
  /var/log/procps-helperd.log*
```
