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
