# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{
  inputs,
  lib,
  flake-parts-lib,
  ...
}: {
  options.perSystem = flake-parts-lib.mkPerSystemOption (_: let
    t = lib.types;
  in {
    options.procps-darwin = {
      rustToolchain = lib.mkOption {
        type = t.package;
        description = "Rust toolchain for building";
      };

      rustfmtNightly = lib.mkOption {
        type = t.package;
        description = "Nightly rustfmt for unstable options";
      };

      rustfmtBin = lib.mkOption {
        type = t.str;
        description = "Path to nightly rustfmt binary";
      };

      wrapscallion = lib.mkOption {
        type = t.package;
        description = "Commit message linter";
      };
    };
  });

  config.perSystem = {
    pkgs,
    system,
    ...
  }: let
    fenixPkgs = inputs.fenix.packages.${system};
    rustfmtNightly = fenixPkgs.latest.rustfmt;
  in {
    procps-darwin = {
      inherit rustfmtNightly;

      rustToolchain = fenixPkgs.stable.withComponents [
        "cargo"
        "clippy"
        "rust-src"
        "rust-analyzer"
        "rustc"
      ];

      rustfmtBin = lib.getExe' rustfmtNightly "rustfmt";

      wrapscallion = pkgs.callPackage ./wrapscallion/package.nix {};
    };
  };
}
