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

      craneLib = lib.mkOption {
        type = t.raw;
        description = "Crane library configured with the toolchain";
      };

      commonArgs = lib.mkOption {
        type = t.attrsOf t.raw;
        description = "Shared Crane build arguments";
      };

      cargoArtifacts = lib.mkOption {
        type = t.package;
        description = "Pre-built Cargo dependencies for the release package";
      };

      checkArgs = lib.mkOption {
        type = t.attrsOf t.raw;
        description = "Crane build arguments for the checks";
      };

      checkArtifacts = lib.mkOption {
        type = t.package;
        description = "Pre-built Cargo dependencies for the checks";
      };

      wrapscallion = lib.mkOption {
        type = t.package;
        description = "Commit message linter";
      };
    };
  });

  config.perSystem = {
    config,
    pkgs,
    system,
    ...
  }: let
    fenixPkgs = inputs.fenix.packages.${system};
    rustfmtNightly = fenixPkgs.latest.rustfmt;

    rustToolchain = fenixPkgs.stable.withComponents [
      "cargo"
      "clippy"
      "rust-src"
      "rust-analyzer"
      "rustc"
    ];

    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
  in {
    procps-darwin = {
      inherit craneLib rustfmtNightly rustToolchain;

      commonArgs = {
        src = craneLib.cleanCargoSource inputs.self;
        strictDeps = true;
        cargoExtraArgs = "--locked --workspace";
      };

      cargoArtifacts = craneLib.buildDepsOnly config.procps-darwin.commonArgs;

      # The checks build in the dev profile, as `cargo test` does, so the tests
      # run with debug assertions and overflow checks. Sharing the release
      # artifacts would turn both off.
      checkArgs = config.procps-darwin.commonArgs // {CARGO_PROFILE = "dev";};
      checkArtifacts = craneLib.buildDepsOnly config.procps-darwin.checkArgs;

      rustfmtBin = lib.getExe' rustfmtNightly "rustfmt";

      wrapscallion = pkgs.callPackage ./wrapscallion/package.nix {};
    };
  };
}
