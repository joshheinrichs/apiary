use std::fs;
use std::os::unix::fs as unix_fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use clap::Parser;
use bubblewand::SandboxArgs;

#[derive(Parser)]
#[command(
    name = "bubblewand-generator",
    about = "Wrap a package's binaries and desktop entries in a bubblewand sandbox"
)]
struct Cli {
    #[command(flatten)]
    sandbox: SandboxArgs,

    /// Only wrap the named binary (may be repeated; default: all executables)
    #[arg(long = "bin", value_name = "NAME")]
    bins: Vec<String>,

    /// File of paths (one per line) to ro-bind into the sandbox; baked into the wrapper at build time
    #[arg(long = "ro-bind-file", value_name = "FILE")]
    ro_bind_file: Option<PathBuf>,

    /// Source package directory (e.g. ${pkgs.discord})
    source: PathBuf,

    /// Output directory (e.g. $out)
    output: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        eprintln!("bubblewand-generator: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    const BUBBLEWAND: &str = match option_env!("BUBBLEWAND") { Some(s) => s, None => "bubblewand" };
    let bubblewand_bin = PathBuf::from(BUBBLEWAND);
    let mut flags = cli.sandbox.to_cli_args();

    if let Some(ref path_file) = cli.ro_bind_file {
        for line in fs::read_to_string(path_file)?.lines() {
            let p = line.trim();
            if !p.is_empty() {
                flags.push(format!("--ro-bind={}:{}", p, p));
            }
        }
    }

    // 1. Wrap executables
    let src_bin = cli.source.join("bin");
    if src_bin.is_dir() {
        let out_bin = cli.output.join("bin");
        fs::create_dir_all(&out_bin)?;

        for entry in fs::read_dir(&src_bin)? {
            let entry = entry?;
            let src_path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if fs::metadata(&src_path)?.permissions().mode() & 0o111 == 0 {
                continue;
            }
            if !cli.bins.is_empty() && !cli.bins.iter().any(|b| b == name_str.as_ref()) {
                continue;
            }

            let script = wrapper_script(&bubblewand_bin, &flags, &src_path);
            write_executable(&out_bin.join(&name), &script)?;
        }
    }

    // 2. Patch .desktop files
    let src_apps = cli.source.join("share").join("applications");
    if src_apps.is_dir() {
        let out_apps = cli.output.join("share").join("applications");
        fs::create_dir_all(&out_apps)?;

        // Collect bin names up front so patch_desktop is pure
        let src_bins: Vec<String> = if src_bin.is_dir() {
            fs::read_dir(&src_bin)?
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        } else {
            Vec::new()
        };

        for entry in fs::read_dir(&src_apps)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            let patched = patch_desktop(&content, &src_bins, &cli.output.join("bin"));
            fs::write(out_apps.join(entry.file_name()), patched)?;
        }
    }

    // 3. Symlink icons
    let src_icons = cli.source.join("share").join("icons");
    if src_icons.is_dir() {
        let out_icons = cli.output.join("share").join("icons");
        fs::create_dir_all(out_icons.parent().unwrap())?;
        if out_icons.symlink_metadata().is_err() {
            unix_fs::symlink(&src_icons, &out_icons)?;
        }
    }

    Ok(())
}

/// Write a file with executable permissions.
fn write_executable(dest: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(dest, content)?;
    let mut perms = fs::metadata(dest)?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(dest, perms)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Generate a one-line wrapper script that execs bubblewand with the right flags.
fn wrapper_script(bubblewand: &Path, flags: &[String], exe: &Path) -> String {
    let mut args = vec![bubblewand.to_string_lossy().into_owned()];
    args.extend_from_slice(flags);
    args.push("--".into());
    args.push(exe.to_string_lossy().into_owned());
    format!("#!/bin/sh\nexec {} \"$@\"\n", shell_words::join(&args))
}

/// Patch Exec= / TryExec= lines in a .desktop file, replacing any reference to
/// a binary in `src_bins` with its counterpart under `out_bin`.
fn patch_desktop(content: &str, src_bins: &[String], out_bin: &Path) -> String {
    let mut replaced = false;
    let mut out = String::with_capacity(content.len());

    for line in content.lines() {
        let prefix = if line.starts_with("Exec=") {
            Some("Exec=")
        } else if line.starts_with("TryExec=") {
            Some("TryExec=")
        } else {
            None
        };

        if let Some(prefix) = prefix {
            let val = &line[prefix.len()..];
            let exe_end = val.find(|c: char| c.is_ascii_whitespace()).unwrap_or(val.len());
            let exe_token = &val[..exe_end];
            let rest = &val[exe_end..];

            let basename = Path::new(exe_token)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(exe_token);

            if src_bins.iter().any(|b| b == basename) {
                out.push_str(prefix);
                out.push_str(&out_bin.join(basename).to_string_lossy());
                out.push_str(rest);
                replaced = true;
            } else {
                out.push_str(line);
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    if !replaced {
        eprintln!("bubblewand-generator: warning: no Exec= lines replaced in desktop file");
    }
    if !content.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(content: &str, bins: &[&str]) -> String {
        let bins: Vec<String> = bins.iter().map(|s| s.to_string()).collect();
        patch_desktop(content, &bins, Path::new("/out/bin"))
    }

    #[test]
    fn bare_name() {
        assert_eq!(
            patch("Exec=spotify %U\n", &["spotify"]),
            "Exec=/out/bin/spotify %U\n",
        );
    }

    #[test]
    fn full_path() {
        assert_eq!(
            patch("Exec=/nix/store/abc-spotify/bin/spotify --flag\n", &["spotify"]),
            "Exec=/out/bin/spotify --flag\n",
        );
    }

    #[test]
    fn tryexec() {
        assert_eq!(
            patch("TryExec=discord\n", &["discord"]),
            "TryExec=/out/bin/discord\n",
        );
    }

    #[test]
    fn no_args() {
        assert_eq!(
            patch("Exec=spotify\n", &["spotify"]),
            "Exec=/out/bin/spotify\n",
        );
    }

    #[test]
    fn unknown_exe_is_left_alone() {
        let input = "Exec=something-else %U\n";
        assert_eq!(patch(input, &["spotify"]), input);
    }

    #[test]
    fn other_lines_are_unchanged() {
        let input = "[Desktop Entry]\nName=Spotify\nExec=spotify\nIcon=spotify\n";
        assert_eq!(
            patch(input, &["spotify"]),
            "[Desktop Entry]\nName=Spotify\nExec=/out/bin/spotify\nIcon=spotify\n",
        );
    }

    #[test]
    fn multiple_sections() {
        let input = "Exec=spotify\n[Desktop Action]\nExec=spotify --uri\n";
        assert_eq!(
            patch(input, &["spotify"]),
            "Exec=/out/bin/spotify\n[Desktop Action]\nExec=/out/bin/spotify --uri\n",
        );
    }

    #[test]
    fn no_trailing_newline_preserved() {
        assert_eq!(
            patch("Exec=spotify", &["spotify"]),
            "Exec=/out/bin/spotify",
        );
    }

    #[test]
    fn wrapper_script_bare_paths() {
        assert_eq!(
            wrapper_script(
                Path::new("/out/bin/bubblewand"),
                &["--gui".into(), "--network".into()],
                Path::new("/nix/store/abc/bin/spotify"),
            ),
            "#!/bin/sh\nexec /out/bin/bubblewand --gui --network -- /nix/store/abc/bin/spotify \"$@\"\n",
        );
    }

    #[test]
    fn wrapper_script_quotes_special_chars() {
        let out = wrapper_script(
            Path::new("/out/bin/bubblewand"),
            &["--dbus-talk=org.freedesktop.portal.*".into()],
            Path::new("/nix/store/abc/bin/app"),
        );
        // shell-words quotes anything with shell metacharacters
        assert!(out.starts_with("#!/bin/sh\n"));
        assert!(out.contains("--dbus-talk=org.freedesktop.portal"));
        assert!(out.ends_with("-- /nix/store/abc/bin/app \"$@\"\n"));
    }
}
