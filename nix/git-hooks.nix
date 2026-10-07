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
        # A golden file records a command's output byte for byte.
        end-of-file-fixer = {
          enable = true;
          excludes = ["^crates/procps-harness/golden/"];
        };
        trim-trailing-whitespace = {
          enable = true;
          excludes = ["^crates/procps-harness/golden/"];
        };

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
          before = ["workspace-clippy"];
        };

        workspace-clippy = {
          enable = true;
          name = "clippy";
          entry = "${pkgs.bash}/bin/bash -c 'if [ \"$PROCPS_DARWIN_DEV_SHELL\" = \"$(${lib.getExe config.procps-darwin.devShellDigest})\" ]; then just clippy; else nix develop --command just clippy; fi'";
          language = "system";
          pass_filenames = false;
          always_run = true;
        };

        flake-check = {
          enable = true;
          name = "nix flake check";
          entry = "${pkgs.bash}/bin/bash -c 'nix flake check \"git+file://$(git rev-parse --show-toplevel)?rev=\${PRE_COMMIT_TO_REF:-$(git rev-parse \"\${PRE_COMMIT_LOCAL_BRANCH:-HEAD}\")}\"'";
          language = "system";
          pass_filenames = false;
          always_run = true;
          stages = ["pre-push"];
        };
      };
    };
  };
}
