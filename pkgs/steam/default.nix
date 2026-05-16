{ pkgs }:
let
  bin = pkgs.writeShellScriptBin "steam" ''
    exec ${pkgs.bubblewrap}/bin/bwrap \
      --dev-bind / / \
      --tmpfs /run \
      --bind /run/user /run/user \
      --bind-try /run/opengl-driver /run/opengl-driver \
      --ro-bind ${pkgs.pkgsi686Linux.mesa} /run/opengl-driver-32 \
      -- ${pkgs.steam}/bin/steam "$@"
  '';
in
pkgs.symlinkJoin {
  name = "steam";
  paths = [ pkgs.steam ];
  postBuild = ''
    rm $out/bin/steam
    ln -s ${bin}/bin/steam $out/bin/steam
  '';
}
