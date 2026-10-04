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

    # Kodi 22 for HDR on Wayland (color-management-v1); nixpkgs still ships 21.
    # The swig override can go once nixpkgs carries 4.5; the in-tree deps stay
    # because nixpkgs' libcrossguid has no pkg-config file and its libdvdnav is
    # older than Kodi accepts.

    # The Python bindings refuse SWIG older than 4.5.
    swig = pkgs.buildPackages.swig.overrideAttrs (
      finalAttrs: _: {
        version = "4.5.1";
        src = pkgs.fetchFromGitHub {
          owner = "swig";
          repo = "swig";
          rev = "v${finalAttrs.version}";
          hash = "sha256-4E27t+ut0XGqiu0pEi8mo/rfDdT2Rb2jaiSu1xufZfU=";
        };
      }
    );

    # Built in-tree by Kodi's own cmake from the archives pinned in
    # tools/depends/target/*/*-VERSION. <NAME>_URL (upper case) points it at a
    # local copy; without one it downloads, which the sandbox refuses.
    inTreeDeps = {
      LIBDVDCSS = {
        archive = "libdvdcss-1.5.0.tar.bz2";
        sha512 = "439fbd9dae60b9a114d3429a19703478c734e8525ac6852da6f05f72d9ca44ca0ac5e874ab6a10017e7f31869fcd29f01d1bc156c4f0331b4e4abd98ec2f95cd";
      };
      LIBDVDNAV = {
        archive = "libdvdnav-7.0.0.tar.bz2";
        sha512 = "8d12d476e352def9716ecbd7ba184a70dd9bae87dcde669b8647a0ef9ac1991f617c9918c043e159eb353302c4cc50fb71bc591ceaeba1d3b9d47af77fa72c71";
      };
      LIBDVDREAD = {
        archive = "libdvdread-7.0.1.tar.bz2";
        sha512 = "b5390a1acb8fdf6e24188f9259199eb0fef2dd4b332447ba2bfa571f9fc0c3bfad00c1d27a8b8806180a853c04a818b7802d3075ff787142c1a175ad798c6a3d";
      };
      CROSSGUID = {
        archive = "crossguid-ca1bf4b810e2d188d04cb6286f957008ee1b7681.tar.gz";
        sha512 = "f0a80d8e99b10473bcfdfde3d1c5fd7b766959819f0d1c0595ac84ce46db9007a5fbfde9a55aca60530c46cb7f8ef4c7e472c6191559ded92f868589c141ccaf";
      };
    };

    kodi = (pkgs.kodi-wayland.override { inherit (pkgs) ffmpeg; }).overrideAttrs (
      finalAttrs: old: {
        version = "22.0rc1";
        kodiReleaseName = "Piers";
        src = pkgs.fetchFromGitHub {
          owner = "xbmc";
          repo = "xbmc";
          rev = "${finalAttrs.version}-${finalAttrs.kodiReleaseName}";
          hash = "sha256-zIQdK3Rj+TLSopJ1R10kEDlHkrgHa36HFN4VszSUJLM=";
        };
        patches = [ ];
        # nixpkgs pins these as source trees for 21; 22 takes inTreeDeps instead.
        libdvdcss = "";
        libdvdnav = "";
        libdvdread = "";

        cmakeFlags =
          lib.filter (
            f:
            f != "-DENABLE_INTERNAL_CROSSGUID=OFF"
            && !lib.hasPrefix "-DSWIG_EXECUTABLE=" f
            && !lib.hasPrefix "-Dlibdvd" f
          ) old.cmakeFlags
          ++ [
            "-DSWIG_EXECUTABLE=${swig}/bin/swig"
            "-DENABLE_INTERNAL_CROSSGUID=ON"
            # FFmpeg 8 dropped libpostproc; Kodi only uses it to deblock
            # software-decoded legacy codecs.
            "-DDISABLE_FFMPEG_SOURCE_PLUGINS=ON"
          ]
          ++ lib.mapAttrsToList (
            name: dep:
            "-D${name}_URL=${
              pkgs.fetchurl {
                url = "https://mirrors.kodi.tv/build-deps/sources/${dep.archive}";
                inherit (dep) sha512;
              }
            }"
          ) inTreeDeps;

        # Kodi 22 configures for Ninja; nixpkgs' checkPhase drives make.
        checkPhase = lib.replaceStrings [ "make -j $NIX_BUILD_CORES" ] [ "ninja -j $NIX_BUILD_CORES" ] old.checkPhase;

        nativeBuildInputs = old.nativeBuildInputs ++ [
          pkgs.meson
          pkgs.ninja
        ];
        buildInputs = old.buildInputs ++ [
          pkgs.exiv2
          pkgs.fmt
          pkgs.pcre2
          pkgs.nlohmann_json
        ];
      }
    );
    # Must be kodi's own addon set: requiredKodiAddons filters on
    # `kodiAddonFor == kodi`, so addons built against any other Kodi (pkgs.kodiPackages,
    # the X11 build) are dropped from the closure without a word.
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
    defaults = {
      "lookandfeel.skin" = arcticFuse.namespace;
    }
    // settings;

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
      [
        (
          with kodi.pythonPackages;
          makePythonPath [
            pillow
            pycryptodome
          ]
        )
      ]
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
