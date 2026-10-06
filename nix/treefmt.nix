{inputs, ...}: {
  imports = [inputs.treefmt-nix.flakeModule];

  perSystem = {
    config,
    pkgs,
    ...
  }: let
    inherit (pkgs) lib;
  in {
    treefmt.config = {
      programs = {
        actionlint.enable = true;
        alejandra.enable = true;
        deadnix.enable = true;
        # Markdown is linted by the custom `markdownlint` formatter below.
        mdformat.enable = false;
        nixf-diagnose = {
          enable = true;
          variableLookup = true;
        };
        prettier = {
          enable = true;
          settings.proseWrap = "always";
        };
        rustfmt = {
          enable = true;
          package = config.procps-darwin.rustfmtNightly;
        };
        statix.enable = true;
        zizmor.enable = true;
      };

      projectRootFile = "flake.nix";

      settings.formatter = {
        zizmor.options = ["--persona" "pedantic"];

        markdownlint = {
          command = lib.getExe pkgs.markdownlint-cli2;
          options = ["--fix" "--"];
          includes = ["*.md"];
        };
      };
    };
  };
}
