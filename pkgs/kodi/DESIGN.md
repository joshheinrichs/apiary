# kodi — design

Kodi 22 (wayland, currently 22.0rc1) with the Arctic Fuse 3 skin, for the TV seat.
Kodi 22 is the first release with HDR on Wayland, through color-management-v1;
nixpkgs still ships 21, so the package overrides `kodi-wayland`.

## What is built and what is state

`KODI_HOME` is built, not copied: a shallow tree of real directories over
symlinks into Kodi's own `share/kodi`, so two files can be replaced without
duplicating the rest. Those two files are what make a fresh profile come up
correct:

- `system/addon-manifest.xml` — every addon in the closure is added as
  `<addon optional="true">`. Kodi files newly discovered addons into its
  database **disabled** unless they are named here
  (`CAddonDatabase::SyncInstalled`), so without this the skin is present, unused
  and silently ignored. *Optional*, not mandatory: `CAddonMgr::Init` aborts
  startup outright if a mandatory addon is missing.
- `system/settings/settings.xml` — the skin is a profile setting, so the only
  declarative place to name it is the setting's `<default>`, which a fresh
  profile inherits.

Everything else in `~/.kodi` is Kodi's: the library database, watched state,
skin settings, `sources.xml`.

## The one reconciler

`script.skinvariables` generates include XML into `special://skin` — the active
skin's own folder — so a skin on a store path cannot work. The launcher mirrors
the skin into `~/.kodi/addons` when the store path moves and leaves Kodi to own
it after that. Its dependencies write to `addon_data` instead and stay read-only
in the store.

## Things that cost time

- Addons must be built from `kodi-wayland.packages`, not `pkgs.kodiPackages`.
  `requiredKodiAddons` filters on `kodiAddonFor == kodi`, and the X11 build is a
  different `kodi`, so addons built against the wrong set are dropped from the
  wrapper with no error and no addon.
- `script.module.pil` is not on the Kodi mirror for any release. Kodi ships a
  stub addon and expects the distro to supply pillow on `PYTHONPATH`.
- `script.skinvariables` 2.2.2 is newer than anything on the mirror; it comes
  from GitHub. The resource addons are only on the mirror, not under
  `jurialmunkey`.
- Kodi's wayland `app_id` is `Kodi`, capitalised — it is `APP_NAME` from
  `version.txt`, not the binary name.

## Editing Kodi's XML

Both patched files are edited with `xmlstarlet` over XPath, not string
substitution: `-u '//setting[@id="…"]/default'` for a setting, `-s /addons` for
a manifest entry. It reformats the document — comments and elements survive
intact, whitespace does not — which Kodi's parser does not care about. Because
`ed -u` succeeds silently when its XPath matches nothing, every requested
setting id is first checked with a `count()` select so a typo fails the build
instead of quietly doing nothing.

## What sources.xml cannot say

Declaring a source is not the same as assigning it content. A path's content
type and scraper live in the video database (`MyVideos*.db`), not in any file
the build can write, so "This directory contains: Movies" is a one-time step in
the UI. After that it is profile state like watched flags, and the declared
source keeps pointing at the same path across rebuilds.

## Building Kodi 22 on nixpkgs' 21 expression

- It builds against nixpkgs' current `ffmpeg`. nixpkgs pins Kodi 21 to
  `ffmpeg_6`, which 22 rejects. FFmpeg 8 dropped libpostproc, so
  `DISABLE_FFMPEG_SOURCE_PLUGINS=ON`. That only loses deblocking for
  software-decoded legacy codecs.
- The SWIG Python bindings refuse anything older than 4.5.
- crossguid and libdvd{css,read,nav} are built in-tree from the archives pinned
  in `tools/depends/target`. Kodi takes a local copy through `-D<NAME>_URL`
  with the name in **upper case**. The lower-case `libdvdcss_URL` flags that
  nixpkgs passes are silently ignored, so Kodi tries to download and the
  sandbox stops it.
- 22 configures for Ninja, so the inherited `make kodi-test` check fails
  after a complete build.
