# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{inputs, ...}: {
  perSystem = {pkgs, ...}: {
    checks.reuse = pkgs.runCommandLocal "reuse-lint" {} ''
      cd ${inputs.self}
      ${pkgs.lib.getExe pkgs.reuse} lint
      touch $out
    '';
  };
}
