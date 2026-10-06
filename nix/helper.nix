# SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
#
# SPDX-License-Identifier: GPL-3.0-or-later
{
  inputs,
  lib,
  self,
  ...
}: let
  label = "io.github.underwhelmingperformance.procps-helperd";
  plistName = "${label}.plist";
  log = "/var/log/procps-helperd.log";
  newsyslog = ../packaging/newsyslog/procps-helperd.conf;

  # The launchd job, apart from the program arguments, which differ between
  # the nix-darwin module's job and the standalone job.
  serviceConfig = {
    Label = label;
    Sockets.Listeners = {
      SockPathName = "/var/run/procps-helperd.sock";
      SockPathMode = 438; # 0666
      SockType = "stream";
    };
    StandardErrorPath = log;
  };

  standalone =
    serviceConfig
    // {
      ProgramArguments = ["/usr/local/libexec/procps-helperd"];
    };
in {
  flake.darwinModules.default = {
    config,
    pkgs,
    ...
  }: let
    cfg = config.services.procps-helperd;
  in {
    options.services.procps-helperd = {
      enable = lib.mkEnableOption ''
        procps-helperd, which reads other users' process data for the procps
        tools
      '';

      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        defaultText = lib.literalExpression "procps-darwin.packages.\${system}.default";
        description = "The package that contains procps-helperd.";
      };
    };

    config = lib.mkIf cfg.enable {
      launchd.daemons.procps-helperd = {
        command = lib.getExe' cfg.package "procps-helperd";
        inherit serviceConfig;
      };

      environment.etc."newsyslog.d/procps-helperd.conf".source = newsyslog;

      # The log records which users ran the tools and when. launchd would create
      # it with mode 0644, so create it first, readable only by root and admin.
      system.activationScripts.extraActivation.text = ''
        touch ${log}
        chown root:admin ${log}
        chmod 640 ${log}
      '';
    };
  };

  perSystem = {
    config,
    pkgs,
    system,
    ...
  }: let
    darwin = inputs.nix-darwin.lib.darwinSystem {
      modules = [
        self.darwinModules.default
        {
          nixpkgs.hostPlatform = system;
          services.procps-helperd.enable = true;
          system.stateVersion = 6;
        }
      ];
    };

    python = pkgs.python3.interpreter;
  in {
    packages.helper-plist =
      pkgs.writeText plistName (lib.generators.toPlist {escape = true;} standalone + "\n");

    checks = {
      helper-plist = pkgs.runCommand "helper-plist" {} ''
        diff -u ${config.packages.helper-plist} ${../packaging/launchd + "/${plistName}"}
        touch $out
      '';

      # nix-darwin runs the module's command through
      # `/bin/sh -c '/bin/wait4path /nix/store && exec …'`, so the program
      # arguments of the module's job differ from the standalone job's.
      helper-module = pkgs.runCommand "helper-module" {} ''
        ${python} ${./helper-module.py} \
          ${config.packages.helper-plist} \
          ${pkgs.writeText plistName darwin.config.environment.launchDaemons.${plistName}.text} \
          ${lib.getExe' config.packages.default "procps-helperd"}
        diff -u ${newsyslog} ${darwin.config.environment.etc."newsyslog.d/procps-helperd.conf".source}
        grep -F 'chmod 640 ${log}' ${pkgs.writeText "activation" darwin.config.system.activationScripts.extraActivation.text}
        touch $out
      '';
    };
  };
}
