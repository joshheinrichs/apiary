{ pkgs, bubblewand }:
pkgs.lib.makeOverridable (
  {
    extraArgs ? [ "--pasta-tcp=127.0.0.1/8384" ],
  }:
  let
    closure = pkgs.closureInfo { rootPaths = [ pkgs.syncthing ]; };
  in
  pkgs.runCommand "bubbled-syncthing"
    {
      nativeBuildInputs = [ bubblewand.generator ];
    }
    ''
      bubblewand-generator install \
        --persist-home=syncthing \
        --pasta \
        --bin=syncthing \
        "--ro-bind-file=${closure}/store-paths" \
        ${pkgs.lib.escapeShellArgs extraArgs} \
        ${pkgs.syncthing} \
        $out
    ''
) { }
