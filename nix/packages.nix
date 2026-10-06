# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
_: {
  perSystem = {
    config,
    pkgs,
    ...
  }: let
    inherit (config.procps-darwin) craneLib commonArgs cargoArtifacts;
  in {
    packages.default = craneLib.buildPackage (commonArgs
      // {
        inherit cargoArtifacts;
        doCheck = false;
        meta = {
          description = "procps-ng's ps, top, pgrep and pkill for macOS";
          homepage = "https://github.com/underwhelmingperformance/procps-darwin";
          license = pkgs.lib.licenses.gpl3Plus;
          platforms = pkgs.lib.platforms.darwin;
        };
      });
  };
}
