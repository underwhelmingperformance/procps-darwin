# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
_: {
  perSystem = {
    config,
    pkgs,
    ...
  }: let
    inherit (pkgs) lib;
    inherit (config.procps-darwin) rustToolchain rustfmtNightly rustfmtBin;
  in {
    # Git resolves relative hooks paths against each worktree, where .git may
    # be a file. Use the absolute shared hooks path so linked worktrees run hooks.
    devShells.default = config.pre-commit.devShell.overrideAttrs (previous: {
      nativeBuildInputs =
        previous.nativeBuildInputs
        ++ [
          rustToolchain
          rustfmtNightly
          pkgs.just
          pkgs.reuse
        ];

      RUSTFMT = rustfmtBin;

      shellHook =
        previous.shellHook
        + ''
          ${lib.getExe pkgs.git} config --local core.hooksPath \
            "$(${lib.getExe pkgs.git} rev-parse --path-format=absolute --git-common-dir)/hooks"
        '';
    });
  };
}
