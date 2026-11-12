{
  # git ls-remote https://github.com/NixOS/nixpkgs nixos-unstable
  nixpkgs-src = builtins.fetchGit {
    url = "https://github.com/NixOS/nixpkgs";
    rev = "177d0540942fb205af6993b1d5fa0b57799e09f8";
    ref = "nixos-unstable";
    shallow = true;
  };
  # git ls-remote https://github.com/xddxdd/nix-cachyos-kernel release
  nix-cachyos-kernel-src = builtins.fetchGit {
    url = "https://github.com/xddxdd/nix-cachyos-kernel";
    rev = "53532f14476a597abeaa67bca19f2c009a043fb6";
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
    rev = "ab97a5d09e0f2aa7339cfd35ec5824fd6653e27c";
    ref = "master";
    shallow = true;
  };
}
