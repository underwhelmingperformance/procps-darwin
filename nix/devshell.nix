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

    # Each file that defines this shell, with its path relative to the flake,
    # in the order of those paths. `dev-shell-digest` hashes the same files, in
    # the same order and format.
    shellFiles = let
      root = toString ../.;
      relative = path: lib.removePrefix "${root}/" (toString path);
    in
      lib.sort (a: b: a.name < b.name) (map (path: {
        name = relative path;
        inherit path;
      }) ([../flake.nix ../flake.lock] ++ lib.filesystem.listFilesRecursive ./.));
  in {
    # Git resolves relative hooks paths against each worktree, where .git may
    # be a file. Use the absolute shared hooks path so linked worktrees run hooks.
    devShells.default = config.pre-commit.devShell.overrideAttrs (previous: {
      nativeBuildInputs =
        previous.nativeBuildInputs
        ++ [
          rustToolchain
          rustfmtNightly
          pkgs.cargo-audit
          pkgs.cargo-deny
          pkgs.just
          pkgs.reuse
        ];

      RUSTFMT = rustfmtBin;

      # Nix computes the digest when it evaluates the shell, so a shell that
      # direnv restores from its cache keeps the digest of the files that it
      # was built from. The clippy hook compares it with the digest of the
      # current files, and uses this shell's toolchain only while the two match.
      PROCPS_DARWIN_DEV_SHELL = builtins.hashString "sha256" (lib.concatMapStrings
        (file: "${file.name}\n${builtins.readFile file.path}\n")
        shellFiles);

      shellHook =
        previous.shellHook
        + ''
          ${lib.getExe pkgs.git} config --local core.hooksPath \
            "$(${lib.getExe pkgs.git} rev-parse --path-format=absolute --git-common-dir)/hooks"
        '';
    });
  };
}
