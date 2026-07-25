{ pkgs }:

# Slippi's mainline Dolphin fork, built from source (see INTENT.md). This is the
# netplay build — the emulator you actually need to play Slippi online; the
# launcher is deliberately not packaged. Playback (replay) is the same source
# with -DSLIPPI_PLAYBACK=true and is left as a sibling for later.
#
# Modelled on nixpkgs' dolphin-emu derivation, overridden with the fork's
# source, CMake flags, extra system libs the older tree wants, and the Rust
# SlippiRustExtensions subproject (cargo-vendored so the CMake build's `cargo`
# invocation works offline).

let
  inherit (pkgs) lib rustPlatform;

  version = "4.0.0-mainline-beta.19";

  src = pkgs.fetchFromGitHub {
    owner = "project-slippi";
    repo = "dolphin";
    rev = "v${version}";
    fetchSubmodules = true;
    hash = "sha256-LX9AMY6ipja6A+fxVlrqrngG6Kac7vMy/A57vr3KSIM=";
  };

  # SlippiRustExtensions (Externals/SlippiRustExtensions) is a Rust workspace the
  # Dolphin CMake build compiles via cargo. Vendor its crates (all crates.io, no
  # git deps) so the build runs with no network.
  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit src;
    name = "slippi-dolphin-${version}-cargo-deps";
    sourceRoot = "${src.name}/Externals/SlippiRustExtensions";
    hash = "sha256-XqsYL70ls7PrOjx1A8LKn7WjbNwLSgBAoqrO2gIQFxI=";
  };
in
pkgs.dolphin-emu.overrideAttrs (old: {
  pname = "slippi-dolphin";
  inherit version src;

  # The fork tracks an older Dolphin master and links more system libraries than
  # current upstream. Extend, don't replace, the nixpkgs dependency set.
  nativeBuildInputs = old.nativeBuildInputs ++ [
    rustPlatform.cargoSetupHook
    pkgs.cargo
    pkgs.rustc
  ];

  # The fork tracks an older Dolphin master and expects several libraries from
  # its bundled Externals (its CI installs no -dev package for them). Passing
  # nixpkgs' newer system copies breaks the build (e.g. fmt moved `fmt::localtime`
  # out of the core header), so drop those and let CMake use the vendored ones.
  buildInputs =
    let
      vendored = [ "fmt" ];
      keep = p: !(builtins.elem (lib.getName p) vendored);
    in
    (builtins.filter keep old.buildInputs) ++ (with pkgs; [
      soundtouch
      libsoundio
      portaudio
      readline
      libao
    ]);

  inherit cargoDeps;
  cargoRoot = "Externals/SlippiRustExtensions";

  # Netplay build. We do a normal FHS install (Sys in $out/share/dolphin-emu).
  cmakeFlags = [
    (lib.cmakeBool "SLIPPI_PLAYBACK" false)
    (lib.cmakeFeature "DISTRIBUTOR" "NixOS")
    (lib.cmakeFeature "CMAKE_POLICY_VERSION_MINIMUM" "3.10")
    (lib.cmakeFeature "DOLPHIN_WC_DESCRIBE" "v${version}")
    (lib.cmakeFeature "DOLPHIN_WC_REVISION" "v${version}")
    (lib.cmakeFeature "DOLPHIN_WC_BRANCH" "slippi")
  ];

  # Unlike upstream Dolphin, the fork's Linux CreateSysDirectoryPath ignores the
  # compiled data dir and hardcodes Sys to ~/.config/SlippiOnline/Sys — a mutable
  # path the launcher normally populates. With no launcher, boot fails (empty
  # GameSettings → the "not a copy of Melee" gate). Patch that branch to use the
  # store install path (DATA_DIR, already $out/share/dolphin-emu/) instead.
  patches = (old.patches or [ ]) ++ [ ./sys-dir-store-path.patch ];

  # nixpkgs' preConfigure reads a COMMIT file its own src.postFetch wrote; we
  # replaced src, so drop it and pass the version via cmakeFlags above instead.
  preConfigure = "";

  # The Slippi Rust extensions build as a shared lib that the emulator dlopens by
  # soname, but `make install` doesn't ship it. Install it and rpath $out/lib.
  postInstall = (old.postInstall or "") + ''
    rustlib=$(find . "$NIX_BUILD_TOP" -name 'libslippi_rust_extensions.so' -print -quit 2>/dev/null)
    if [ -z "$rustlib" ]; then
      echo "libslippi_rust_extensions.so not found in build tree" >&2
      exit 1
    fi
    install -Dm755 "$rustlib" "$out/lib/libslippi_rust_extensions.so"
  '';

  postFixup = (old.postFixup or "") + ''
    for b in $out/bin/.dolphin-emu-wrapped $out/bin/.dolphin-emu-nogui-wrapped; do
      [ -e "$b" ] && patchelf --add-rpath "$out/lib" "$b"
    done
  '';

  meta = (old.meta or { }) // {
    description = "Slippi's mainline Dolphin fork (netplay) for Melee online";
    homepage = "https://slippi.gg";
    mainProgram = "dolphin-emu";
  };
})
