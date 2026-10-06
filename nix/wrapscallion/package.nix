# wrapscallion commit message linter, packaged from GitHub release binaries.
{
  fetchurl,
  lib,
  stdenvNoCC,
}: let
  sources = lib.importJSON ./sources.json;
  inherit (stdenvNoCC.hostPlatform) system;

  platform =
    sources.platforms.${system}
    or (throw "wrapscallion: unsupported system ${system}");
in
  stdenvNoCC.mkDerivation {
    pname = "wrapscallion";
    inherit (sources) version;

    src = fetchurl {
      inherit (platform) url hash;
    };

    dontUnpack = true;

    installPhase = ''
      runHook preInstall
      install -Dm755 "$src" "$out/bin/wrapscallion"
      runHook postInstall
    '';

    doInstallCheck = true;

    installCheckPhase = ''
      runHook preInstallCheck
      "$out/bin/wrapscallion" --help >/dev/null
      runHook postInstallCheck
    '';

    meta = {
      description = "Linter for Conventional Commit messages and 72-column bodies";
      homepage = "https://github.com/underwhelmingperformance/wrapscallion";
      license = lib.licenses.mit;
      platforms = builtins.attrNames sources.platforms;
      mainProgram = "wrapscallion";
    };
  }
