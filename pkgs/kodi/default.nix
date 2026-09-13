{ pkgs }:
pkgs.lib.makeOverridable (
  {
    # Video sources to declare, as { name, path }. sources.xml lives in the profile
    # and Kodi only seeds RssFeeds/favourites/Lircmap from the install tree, so the
    # launcher reconciles this one.
    videoSources ? [ ],
    # Kodi setting id -> default value, patched into the install tree's settings
    # schema. A fresh profile inherits these and the user can still change them.
    settings ? { },
  }:
  let
    inherit (pkgs) lib;

    # Must be kodi-wayland's own addon set: requiredKodiAddons filters on
    # `kodiAddonFor == kodi`, so addons built against pkgs.kodiPackages (the X11
    # build) are dropped from the closure without a word.
    kodi = pkgs.kodi-wayland;
    kp = kodi.packages;

    mirror =
      namespace: version: hash:
      pkgs.fetchzip {
        url = "https://mirrors.kodi.tv/addons/${lib.toLower kp.rel}/${namespace}/${namespace}-${version}.zip";
        inherit hash;
      };

    qrcode = kp.buildKodiAddon rec {
      pname = "qrcode";
      namespace = "script.module.qrcode";
      version = "6.1.0+matrix.3";

      src = mirror namespace version "sha256-nO0bnXinKQfNDrx1Kd610uS8ejeAAq9PcQg+ZiOA3Gw=";

      propagatedBuildInputs = [ kp.six ];

      passthru.pythonPath = "lib";
    };

    skinvariables = kp.buildKodiAddon rec {
      pname = "skinvariables";
      namespace = "script.skinvariables";
      version = "2.2.2";

      src = pkgs.fetchFromGitHub {
        owner = "jurialmunkey";
        repo = namespace;
        rev = "v${version}";
        hash = "sha256-HmJ8+6kkw3vxXh/PWuxpsdMp/2JBmOW1wDDuGyO/0Zw=";
      };

      propagatedBuildInputs = [
        kp.jurialmunkey
        kp.infotagger
      ];
    };

    themoviedb-helper = kp.buildKodiAddon rec {
      pname = "themoviedb-helper";
      namespace = "plugin.video.themoviedb.helper";
      version = "6.17.1";

      src = pkgs.fetchFromGitHub {
        owner = "jurialmunkey";
        repo = namespace;
        rev = "v${version}";
        hash = "sha256-gm68w13PZSJUtZ08D+nzh+X8Q3GXt2YgELz9lZsmZwY=";
      };

      propagatedBuildInputs = [
        kp.requests
        kp.signals
        kp.jurialmunkey
        kp.infotagger
        qrcode
      ];
    };

    weathericons-white = kp.buildKodiAddon rec {
      pname = "weathericons-white";
      namespace = "resource.images.weathericons.white";
      version = "0.0.6";

      src = mirror namespace version "sha256-aYlYBo+KnMPuNUsj/CJcgEwRXWdzFrrdWIMpKdOvHns=";
    };

    studios-coloured = kp.buildKodiAddon rec {
      pname = "studios-coloured";
      namespace = "resource.images.studios.coloured";
      version = "0.0.24";

      src = mirror namespace version "sha256-f9abo738scAsqHRi+EnrWPMm+oxbKf6TrfkssoH3yME=";
    };

    arcticFuse = kp.buildKodiAddon rec {
      pname = "arctic-fuse";
      namespace = "skin.arctic.fuse.3";
      version = "3.2.19";

      src = pkgs.fetchFromGitHub {
        owner = "jurialmunkey";
        repo = namespace;
        rev = "v${version}";
        hash = "sha256-+o3PfURLMVz8pvze1IB74ZETA7TNph/YxJsWUq4h2FU=";
      };

      propagatedBuildInputs = [
        skinvariables
        kp.texturemaker
        themoviedb-helper
        weathericons-white
        studios-coloured
        kp.robotocjksc
      ];
    };

    closure = kp.requiredKodiAddons [ arcticFuse ];
    # The skin is the one addon that cannot live in the store: script.skinvariables
    # generates include XML into special://skin, which is the skin's own folder.
    # Everything it depends on is read-only and stays here.
    storeAddons = lib.filter (a: a != arcticFuse) closure;

    # Kodi files newly discovered addons into its database *disabled* unless they
    # are named in system/addon-manifest.xml (CAddonDatabase::SyncInstalled).
    # Listing them optional enables them on first run without making them
    # mandatory — a mandatory addon that goes missing aborts startup.
    manifest = pkgs.runCommand "kodi-addon-manifest.xml" { } ''
      ${pkgs.xmlstarlet}/bin/xmlstarlet ed \
        ${
          lib.concatMapStringsSep " \\\n      " (a: ''
            -s /addons -t elem -n addon -v ${a.namespace} \
              -i '/addons/addon[last()]' -t attr -n optional -v true'') closure
        } \
        ${kodi}/share/kodi/system/addon-manifest.xml > $out
    '';

    # Every Kodi setting is profile state, so the only declarative place to pin one
    # is its default in the install tree's schema. The skin is pinned here rather
    # than anywhere else for exactly that reason.
    defaults = { "lookandfeel.skin" = arcticFuse.namespace; } // settings;

    settingsFile = pkgs.runCommand "kodi-settings.xml" { } ''
      schema=${kodi}/share/kodi/system/settings/settings.xml
      xmlstarlet=${pkgs.xmlstarlet}/bin/xmlstarlet

      # A misspelled id would otherwise update nothing and pass silently.
      ${lib.concatMapStringsSep "\n    " (id: ''
        [ "$($xmlstarlet sel -t -v 'count(//setting[@id="${id}"]/default)' "$schema")" = 1 ] \
          || { echo "no such setting: ${id}" >&2; exit 1; }
      '') (lib.attrNames defaults)}

      $xmlstarlet ed \
        ${
          lib.concatMapStringsSep " \\\n      " (
            id: "-u '//setting[@id=\"${id}\"]/default' -v ${lib.escapeShellArg defaults.${id}}"
          ) (lib.attrNames defaults)
        } \
        "$schema" > $out
    '';

    sourceXml =
      s:
      lib.concatStringsSep "\n" [
        "    <source>"
        "      <name>${s.name}</name>"
        "      <path pathversion=\"1\">${s.path}</path>"
        "      <allowsharing>true</allowsharing>"
        "    </source>"
      ];

    sourcesFile = pkgs.writeText "sources.xml" (
      lib.concatStringsSep "\n" (
        [
          "<sources>"
          "  <video>"
        ]
        ++ map sourceXml videoSources
        ++ [
          "  </video>"
          "</sources>"
          ""
        ]
      )
    );

    # KODI_HOME, built rather than copied: a shallow tree of real directories over
    # symlinks into the store, so the two patched files can replace their originals
    # without duplicating the rest of Kodi's share tree.
    home = pkgs.runCommand "kodi-home" { } ''
      mkdir -p $out/addons $out/system/settings

      for entry in ${kodi}/share/kodi/*; do
        case "$(basename "$entry")" in
          addons | system) ;;
          *) ln -s "$entry" $out/ ;;
        esac
      done

      ln -s ${kodi}/share/kodi/addons/* $out/addons/
      ${lib.concatMapStrings (a: ''
        ln -s ${a}${kp.addonDir}/${a.namespace} $out/addons/
      '') storeAddons}

      for entry in ${kodi}/share/kodi/system/*; do
        case "$(basename "$entry")" in
          addon-manifest.xml | settings) ;;
          *) ln -s "$entry" $out/system/ ;;
        esac
      done
      ln -s ${manifest} $out/system/addon-manifest.xml

      ln -s ${kodi}/share/kodi/system/settings/* $out/system/settings/
      rm $out/system/settings/settings.xml
      ln -s ${settingsFile} $out/system/settings/settings.xml
    '';

    pythonPath = lib.concatStringsSep ":" (
      [ (with kodi.pythonPackages; makePythonPath [ pillow pycryptodome ]) ]
      ++ map (a: "${a}${kp.addonDir}/${a.namespace}/${a.pythonPath}") (
        lib.filter (a: a ? pythonPath) storeAddons
      )
    );

    skinDir = "${arcticFuse}${kp.addonDir}/${arcticFuse.namespace}";
  in
  pkgs.writeShellScriptBin "kodi" ''
    set -eu

    # Reconcile the one mutable piece: the skin has to sit somewhere it can write
    # its generated includes, so mirror it out of the store when the store path
    # moves and leave Kodi to own it after that.
    target="''${HOME}/.kodi/addons/${arcticFuse.namespace}"
    if [ "$(cat "$target/.store-path" 2>/dev/null || true)" != "${skinDir}" ]; then
      rm -rf "$target"
      mkdir -p "$(dirname "$target")"
      cp -r --no-preserve=mode,ownership "${skinDir}" "$target"
      chmod -R u+w "$target"
      printf '%s' "${skinDir}" > "$target/.store-path"
    fi
    ${lib.optionalString (videoSources != [ ]) ''

      # Sources are declared here, not in the UI: overwrite on every launch so the
      # built file is what Kodi reads, whatever the UI last wrote.
      userdata="''${HOME}/.kodi/userdata"
      mkdir -p "$userdata"
      if ! cmp -s ${sourcesFile} "$userdata/sources.xml"; then
        install -m 644 ${sourcesFile} "$userdata/sources.xml"
      fi
    ''}
    export KODI_HOME=${home}
    export PYTHONPATH="${pythonPath}''${PYTHONPATH:+:$PYTHONPATH}"
    exec ${kodi}/bin/kodi "$@"
  ''
) { }
