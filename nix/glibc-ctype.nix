# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{
  lib,
  withSystem,
  ...
}: {
  perSystem = {
    config,
    pkgs,
    ...
  }: let
    # The reference container's procps-ng links the Linux glibc of this
    # nixpkgs, so ps's tables must come from that glibc's patched source.
    # glibc's postPatch edits only build files and refers to Linux packages,
    # which this machine cannot build, so only the patches are applied.
    glibc = withSystem "aarch64-linux" ({pkgs, ...}: pkgs.glibc);
  in {
    packages.glibc-ctype = pkgs.runCommandLocal "glibc_ctype.rs" {nativeBuildInputs = [pkgs.python3];} ''
      mkdir glibc
      tar -xf ${glibc.src} -C glibc --strip-components=1
      for patch in ${lib.escapeShellArgs glibc.patches}; do
        patch --batch --quiet -p1 -d glibc <"$patch"
      done
      python3 -I ${../scripts/generate-glibc-ctype} glibc >$out
    '';

    checks.glibc-ctype = pkgs.runCommandLocal "glibc-ctype" {} ''
      if ! diff -u ${../crates/procps/src/ps/glibc_ctype.rs} ${config.packages.glibc-ctype}; then
        echo "crates/procps/src/ps/glibc_ctype.rs does not match glibc ${glibc.version}." >&2
        echo "Run just glibc-ctype to regenerate it." >&2
        exit 1
      fi

      touch $out
    '';
  };
}
