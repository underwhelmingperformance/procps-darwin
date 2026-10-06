# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{inputs, ...}: {
  imports = [inputs.git-hooks-nix.flakeModule];

  perSystem = {
    config,
    pkgs,
    ...
  }: let
    inherit (pkgs) lib;
  in {
    pre-commit = {
      check.enable = false;

      settings.hooks = {
        check-added-large-files.enable = true;
        check-yaml.enable = true;
        end-of-file-fixer.enable = true;
        trim-trailing-whitespace.enable = true;

        wrapscallion = {
          enable = true;
          description = "Lint Conventional Commit messages and 72-column bodies.";
          entry = "${lib.getExe config.procps-darwin.wrapscallion} --output-format terminal --edit";
          language = "system";
          stages = ["commit-msg"];
        };

        nix-format = {
          enable = true;
          name = "nix fmt";
          entry = "nix fmt";
          language = "system";
          require_serial = true;
          before = ["flake-check"];
        };

        flake-check = {
          enable = true;
          name = "nix flake check";
          entry = "nix flake check";
          language = "system";
          pass_filenames = false;
        };
      };
    };
  };
}
