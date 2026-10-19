{ pkgs, joshheinrichs-ca }:

pkgs.writeShellScriptBin "joshheinrichs-ca-applicator" ''
  set -eu
  exec ${pkgs.wrangler}/bin/wrangler pages deploy ${joshheinrichs-ca} \
    --project-name joshheinrichs --branch main --commit-dirty=true "$@"
''
