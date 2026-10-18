{
  # git ls-remote https://github.com/NixOS/nixpkgs nixos-unstable
  nixpkgs-src = builtins.fetchGit {
    url = "https://github.com/NixOS/nixpkgs";
    rev = "55bfb4d7437bd4e09ab7cba58a43bd68eb310044";
    ref = "nixos-unstable";
    shallow = true;
  };
  # git ls-remote https://github.com/xddxdd/nix-cachyos-kernel release
  nix-cachyos-kernel-src = builtins.fetchGit {
    url = "https://github.com/xddxdd/nix-cachyos-kernel";
    rev = "b1332396df6e880d7e3b6b451c6a74132ce8cf66";
    ref = "release";
    shallow = true;
  };
  # git ls-remote https://github.com/nix-community/lanzaboote v1.2.0
  lanzaboote-src = builtins.fetchGit {
    url = "https://github.com/nix-community/lanzaboote";
    rev = "c1c5edd31802d181c8aa2c71588995d93425d650";
    ref = "refs/tags/v1.2.0";
    shallow = true;
  };
  # git ls-remote https://github.com/nix-community/home-manager master
  home-manager-src = builtins.fetchGit {
    url = "https://github.com/nix-community/home-manager";
    rev = "640725978b99076e18e069f6e7d2959124584f8f";
    ref = "master";
    shallow = true;
  };
}
