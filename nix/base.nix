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

      devShellDigest = lib.mkOption {
        type = t.package;
        description = "Program that prints a digest of the files that define the development shell";
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

    # On aarch64-darwin, the toolchain also has the standard library for aarch64
    # Linux, so that clippy can check the harness code that only Linux compiles.
    rustToolchain = fenixPkgs.combine ([
        (fenixPkgs.stable.withComponents [
          "cargo"
          "clippy"
          "rust-src"
          "rust-analyzer"
          "rustc"
        ])
      ]
      ++ lib.optional (system == "aarch64-darwin")
      fenixPkgs.targets.aarch64-unknown-linux-gnu.stable.rust-std);

    craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
  in {
    procps-darwin = {
      inherit craneLib rustfmtNightly rustToolchain;

      commonArgs = {
        # darwin-proc's build script compiles C files, and Crane's filter drops
        # `.c` and `.h` files.
        src = lib.cleanSourceWith {
          src = inputs.self;
          name = "source";
          filter = path: type:
            craneLib.filterCargoSources path type
            || builtins.match ".*\\.[ch]" path != null;
        };
        strictDeps = true;
        cargoExtraArgs = "--locked --workspace";

        # The cc crate passes `--target=arm64-apple-macosx`. The Nix compiler
        # wrapper warns about any target other than `arm64-apple-darwin`,
        # although LLVM reads both names as this platform.
        NIX_CC_WRAPPER_SUPPRESS_TARGET_WARNING = 1;
      };

      cargoArtifacts = craneLib.buildDepsOnly config.procps-darwin.commonArgs;

      # The checks build in the dev profile, as `cargo test` does, so the tests
      # run with debug assertions and overflow checks. Sharing the release
      # artifacts would turn both off.
      checkArgs = config.procps-darwin.commonArgs // {CARGO_PROFILE = "dev";};
      checkArtifacts = craneLib.buildDepsOnly config.procps-darwin.checkArgs;

      rustfmtBin = lib.getExe' rustfmtNightly "rustfmt";

      wrapscallion = pkgs.callPackage ./wrapscallion/package.nix {};

      devShellDigest = pkgs.writeShellApplication {
        name = "dev-shell-digest";
        runtimeInputs = [pkgs.coreutils pkgs.findutils pkgs.git];
        text = ''
          cd "$(git rev-parse --show-toplevel)"
          # Nix sees only the files that git tracks, so an untracked file such
          # as nix/.DS_Store must not change the digest.
          git ls-files -z flake.nix flake.lock nix |
            LC_ALL=C sort -z |
            while IFS= read -r -d "" file; do
              printf '%s\n' "$file"
              cat "$file"
              printf '\n'
            done |
            sha256sum |
            cut -d ' ' -f 1
        '';
      };
    };
  };
}
