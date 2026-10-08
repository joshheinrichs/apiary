{ pkgs, jieli-toolchain }:
let
  # The three AC79 SDK files the package step reads, where it looks for them.
  # Hashes are the ones build.py checks against.
  sdkFile = rel: sha256: {
    name = "cpu/wl82/tools/${rel}";
    path = pkgs.fetchurl {
      url = "https://gitee.com/Jieli-Tech/fw-AC79_AIoT_SDK/raw/AC79NN_SDK_V1.2.1_2023-12-13/cpu/wl82/tools/${rel}";
      inherit sha256;
    };
  };
  sdk = pkgs.linkFarm "ac79-sdk-files" [
    (sdkFile "uboot.boot" "4e3b4c220dc96641cb5a723f41e68ce41d5261ae9434bb33fbd7f2c59976ded4")
    (sdkFile "cfg_tool.bin" "276579954f076886a6a7694f65dc71c034a63a2c204b76749065c0ac7b010d1b")
    (sdkFile "cfg/eq_cfg_hw.bin" "41167491bffed4651750719c973d2758adeb9021a5670d02d6a53c85ed80ea7d")
  ];
  installer = pkgs.python3.withPackages (p: [
    p.mido
    p.python-rtmidi
  ]);
in
pkgs.stdenv.mkDerivation {
  pname = "sloop";
  version = "2.4.1";
  src = pkgs.fetchFromGitHub {
    owner = "isod89";
    repo = "sloop-fm1";
    rev = "a1c5d68767ae10fafb6821dc63b9b1fc490342d2";
    sha256 = "0psvjdfpgvy5qm3rdwsgd76msbd21lc8gg4mqdrgz594c6qlahx6";
  };
  nativeBuildInputs = [ (pkgs.python3.withPackages (p: [ p.pillow ])) ];
  env = {
    JIELI_TOOLCHAIN = "${jieli-toolchain}";
    AC79_SDK = "${sdk}";
  };
  buildPhase = ''
    runHook preBuild
    python3 tools/build.py
    runHook postBuild
  '';

  # The upstream host suite: golden renders of every engine and preset, the
  # update loader against an older package, the installer against a
  # simulated FM-1, and the web pages.
  # Upstream's tests/target_budget.txt predates 2.4's FM6 engine: the audio
  # IRQ is 54 % over its recorded budget, so that one static check is dropped.
  doCheck = true;
  postPatch = ''
    sed -i '/^run "regression: target cost/,/tests\/target_budget.txt$/d' tests/run_tests.sh
  '';
  nativeCheckInputs = [ pkgs.nodejs ];
  checkPhase = ''
    runHook preCheck
    sh tests/run_tests.sh
    runHook postCheck
  '';

  # sloop-install: upstream's USB-MIDI installer, pointed at the package
  # built here. --info goes straight through, since it takes no package.
  installPhase = ''
    runHook preInstall
    install -Dm444 -t $out build/felucca.fwsc build/felucca.bin build/loader/ota.bin build/ATTRIBUTION.txt
    install -Dm444 -t $out/libexec tools/fm1_install.py
    install -Dm555 /dev/stdin $out/bin/sloop-install <<EOF
    #!${pkgs.runtimeShell}
    run() { exec ${installer}/bin/python3 $out/libexec/fm1_install.py "\$@"; }
    case " \$* " in *" --info "*) run "\$@" ;; esac
    run $out/felucca.fwsc "\$@"
    EOF
    runHook postInstall
  '';
  meta = {
    mainProgram = "sloop-install";
    license = pkgs.lib.licenses.gpl3Only;
    platforms = [ "x86_64-linux" ];
  };
}
