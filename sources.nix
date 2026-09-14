{
  # git ls-remote https://github.com/NixOS/nixpkgs nixos-unstable
  nixpkgs-src = builtins.fetchGit {
    url = "https://github.com/NixOS/nixpkgs";
    rev = "eaad089433ca2bb662274377d33df3d0e51ef28b";
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
    rev = "87b3c74df02a2b2e691a24f3c82774f48a70c1bd";
    ref = "master";
    shallow = true;
  };
}
