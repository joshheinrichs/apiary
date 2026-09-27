{
  # git ls-remote https://github.com/NixOS/nixpkgs nixos-unstable
  nixpkgs-src = builtins.fetchGit {
    url = "https://github.com/NixOS/nixpkgs";
    rev = "e94cb152ed51bd6e24eb4a41f1460252beb52cd2";
    ref = "nixos-unstable";
    shallow = true;
  };
  # git ls-remote https://github.com/xddxdd/nix-cachyos-kernel release
  nix-cachyos-kernel-src = builtins.fetchGit {
    url = "https://github.com/xddxdd/nix-cachyos-kernel";
    rev = "444d135dde71c1de547cf7bfd73e67145e67aebb";
    ref = "release";
    shallow = true;
  };
  # git ls-remote https://github.com/nix-community/home-manager master
  home-manager-src = builtins.fetchGit {
    url = "https://github.com/nix-community/home-manager";
    rev = "7b4c5ec4bedaf1e062bbc1bcaeddbc6bd242aa1b";
    ref = "master";
    shallow = true;
  };
}
