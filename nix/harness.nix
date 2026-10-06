# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{
  lib,
  withSystem,
  ...
}: {
  flake.apps.aarch64-linux.harness-generate = withSystem "aarch64-linux" ({
    config,
    pkgs,
    ...
  }: let
    inherit (config.procps-darwin) craneLib commonArgs;

    harnessArgs = commonArgs // {cargoExtraArgs = "--locked -p procps-harness";};

    harness = craneLib.buildPackage (harnessArgs
      // {
        cargoArtifacts = craneLib.buildDepsOnly harnessArgs;
        doCheck = false;
        meta.mainProgram = "procps-harness";
      });

    # The golden files record this release's behaviour, so a nixpkgs update
    # that changes it must not regenerate them unnoticed.
    procps = assert lib.assertMsg (pkgs.procps.version == "4.0.7") ''
      nixpkgs now provides procps-ng ${pkgs.procps.version}, but the golden files
      come from 4.0.7. Pin procps-ng 4.0.7 in nix/harness.nix.
    '';
      pkgs.procps.overrideAttrs (previous: {
        # nixpkgs builds top with its original look. The golden files record
        # procps-ng's own defaults.
        configureFlags = lib.remove "--disable-modern-top" previous.configureFlags;
      });

    generate = pkgs.writeShellApplication {
      name = "harness-generate";
      runtimeInputs = [harness pkgs.util-linux];
      text = ''
        # procps-ng pads pid columns to the number of digits in pid_max - 1.
        # macOS allocates pids up to 99999, so show procps-ng that limit.
        pid_max=$(mktemp)
        printf '100000\n' >"$pid_max"
        mount --bind "$pid_max" /proc/sys/kernel/pid_max

        # The tests on macOS run as an unprivileged user.
        exec procps-harness generate --tools ${procps}/bin --user 65534:65534 "$@"
      '';
    };
  in {
    type = "app";
    program = lib.getExe generate;
    meta.description = "Write the golden files from procps-ng ${procps.version}";
  });
}
