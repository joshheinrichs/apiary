{ pkgs }:
# JieLi's prebuilt Linux toolchain (clang 4.0.1 for pi32/pi32v2/q32s). The
# binaries expect an FHS loader and a few Python wrappers call
# /usr/bin/env python3; both get patched to the store.
pkgs.stdenv.mkDerivation {
  pname = "jieli-toolchain";
  version = "20250324.1";
  src = pkgs.fetchurl {
    url = "https://jl-update.oss-cn-shenzhen.aliyuncs.com/jieli-linux-toolchains-20250324.1.tar.xz";
    sha256 = "0n2r4algy8d69nhnd2p8y2s3qcbzgafb7lkzn85hypmlrxmmi1pn";
  };
  nativeBuildInputs = [ pkgs.autoPatchelfHook ];
  buildInputs = [
    pkgs.stdenv.cc.cc.lib
    pkgs.python3
  ];
  dontStrip = true;
  installPhase = ''
    runHook preInstall
    cp -r . $out
    runHook postInstall
  '';
  meta = {
    license = pkgs.lib.licenses.unfree;
    sourceProvenance = [ pkgs.lib.sourceTypes.binaryNativeCode ];
    platforms = [ "x86_64-linux" ];
  };
}
