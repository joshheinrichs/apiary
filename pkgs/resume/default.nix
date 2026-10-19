{ pkgs }:

let
  fonts = pkgs.runCommand "resume-fonts" { } ''
    mkdir -p $out
    for face in Regular It Semibold SemiboldIt Bold BoldIt; do
      ln -s ${pkgs.source-serif}/share/fonts/opentype/SourceSerif4-$face.otf $out/
    done
    ln -s ${pkgs.source-code-pro}/share/fonts/opentype/SourceCodePro-Regular.otf $out/
  '';
in
pkgs.runCommand "resume" { nativeBuildInputs = [ pkgs.typst ]; } ''
  mkdir -p $out
  typst compile --ignore-system-fonts --font-path ${fonts} --input out=$out \
    ${./resume.typ} $out/resume.pdf
''
