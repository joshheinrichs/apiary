{ pkgs, seal }:
pkgs.lib.makeOverridable (
  {
    extraArgs ? [ "--publish=tcp:127.0.0.1/8384" ],
  }:
  let
    closure = pkgs.closureInfo { rootPaths = [ pkgs.syncthing ]; };
  in
  pkgs.runCommand "sealed-syncthing"
    {
      nativeBuildInputs = [ seal.generator ];
    }
    ''
      seal-generator install \
        --persist-home=syncthing \
        --net=internet,lan \
        --bin=syncthing \
        "--ro-bind-file=${closure}/store-paths" \
        ${pkgs.lib.escapeShellArgs extraArgs} \
        ${pkgs.syncthing} \
        $out
    ''
) { }
