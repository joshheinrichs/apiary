{ pkgs }:
let
  version = "F13f";
  bios = "X870EAGLEWIFI7.${version}";
  zip = pkgs.fetchurl {
    url = "https://download.gigabyte.com/FileList/BIOS/mb_bios_x870-eagle-wifi7_8arpl325_${pkgs.lib.toLower version}.zip";
    sha256 = "1na9q3qjajy7mrkhcy33z5aar71cd9xhhfcrf0sw13zcgszaqzzl";
  };
in
# MBR + one FAT32 partition holding the image twice: under its own name for
# Q-Flash in the BIOS menu, and as GIGABYTE.bin for the Q-Flash Plus button.
pkgs.runCommand "mainframe-motherboard-firmware-${version}.img"
  {
    nativeBuildInputs = [
      pkgs.unzip
      pkgs.util-linux
      pkgs.dosfstools
      pkgs.mtools
    ];
  }
  ''
    unzip -q ${zip} ${bios}
    truncate -s 128M disk.img
    printf 'label: dos\nlabel-id: 0x42494f53\nstart=2048, type=c\n' | sfdisk -q disk.img
    mkfs.vfat -F 32 --offset 2048 --invariant -n BIOS disk.img $(( (128 * 1024 * 1024 / 512 - 2048) / 2 ))
    mcopy -m -i disk.img@@1M ${bios} ::/${bios}
    mcopy -m -i disk.img@@1M ${bios} ::/GIGABYTE.bin

    for name in ${bios} GIGABYTE.bin; do
      mcopy -i disk.img@@1M ::/$name check
      cmp ${bios} check
      rm check
    done
    mv disk.img $out
  ''
