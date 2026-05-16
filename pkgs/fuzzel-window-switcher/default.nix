{ pkgs }:
pkgs.writeShellApplication {
  name = "fuzzel-window-switcher";
  runtimeInputs = with pkgs; [
    sway
    jq
    fuzzel
  ];
  excludeShellChecks = [ "SC2038" ];
  text = builtins.readFile ./fuzzel-window-switcher.sh;
}
