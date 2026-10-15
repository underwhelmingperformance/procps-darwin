# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{inputs, ...}: {
  perSystem = {config, ...}: let
    inherit (config.procps-darwin) craneLib commonArgs checkArgs checkArtifacts;
    args = checkArgs // {cargoArtifacts = checkArtifacts;};
  in {
    checks = {
      build = config.packages.default;

      clippy = craneLib.cargoClippy (args
        // {
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        });

      # The harness generates the golden files on Linux, where it compiles code
      # that the Darwin build leaves out.
      clippy-linux = let
        linuxArgs =
          checkArgs
          // {
            pname = "procps-harness-linux";
            cargoExtraArgs = "--locked -p procps-harness --target aarch64-unknown-linux-gnu";
          };
      in
        craneLib.cargoClippy (linuxArgs
          // {
            # The toolchain has no linker for Linux, so the dependencies are
            # checked for Linux, not built. Their build scripts and procedural
            # macros still build for Darwin. With `doCheck = false`, Crane's
            # default check arguments leave out `--all-targets`, which the
            # dev-dependencies need.
            cargoArtifacts = craneLib.buildDepsOnly (linuxArgs
              // {
                cargoBuildCommand = "cargoWithProfile check";
                cargoCheckExtraArgs = "--all-targets";
                doCheck = false;
              });
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

      doc = craneLib.cargoDoc (args
        // {
          cargoDocExtraArgs = "--no-deps";
          env.RUSTDOCFLAGS = "--deny warnings";
        });

      tests = craneLib.cargoTest args;

      deny = craneLib.cargoDeny (commonArgs
        // {
          cargoExtraArgs = "";
          cargoDenyChecks = "bans licenses sources";
        });

      # A Nix build cannot fetch the advisory database, so this check reads the
      # pinned advisory-db input. `just audit` reads the live database.
      audit = craneLib.cargoAudit (commonArgs
        // {
          inherit (craneLib.crateNameFromCargoToml {inherit (commonArgs) src;}) pname version;
          inherit (inputs) advisory-db;
        });
    };
  };
}
