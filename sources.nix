{
  # git ls-remote https://github.com/NixOS/nixpkgs nixos-unstable
  nixpkgs-src = builtins.fetchGit {
    url = "https://github.com/NixOS/nixpkgs";
    rev = "331800de5053fcebacf6813adb5db9c9dca22a0c";
    ref = "nixos-unstable";
    shallow = true;
  };
  # git ls-remote https://github.com/xddxdd/nix-cachyos-kernel release
  nix-cachyos-kernel-src = builtins.fetchGit {
    url = "https://github.com/xddxdd/nix-cachyos-kernel";
    rev = "236462fb93cb56e26e6a6801ba5edb6dad66be0d";
    ref = "release";
    shallow = true;
  };
  # git ls-remote https://github.com/nix-community/home-manager master
  home-manager-src = builtins.fetchGit {
    url = "https://github.com/nix-community/home-manager";
    rev = "f384af1bec6423a0d4ba1855917ab948f64e5808";
    ref = "master";
    shallow = true;
  };
}
