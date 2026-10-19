{ pkgs, resume }:

pkgs.runCommand "joshheinrichs-ca" { } ''
  cp -r ${./public} $out
  chmod u+w $out
  cp ${resume}/resume.pdf $out/resume.pdf
''
