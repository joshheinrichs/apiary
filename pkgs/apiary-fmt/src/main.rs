//! apiary-fmt — format the whole repo: nixfmt, statix, cargo fmt.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT: &str = env!("GIT");
const NIXFMT: &str = env!("NIXFMT");
const STATIX: &str = env!("STATIX");
const CARGO: &str = env!("RUST_CARGO");
const CARGO_FMT: &str = env!("RUST_CARGO_FMT");
const RUSTFMT: &str = env!("RUSTFMT");

fn main() -> Result<()> {
    let root = repo_root()?;
    let files = repo_files(&root)?;
    let nix_files: Vec<&String> = files.iter().filter(|f| f.ends_with(".nix")).collect();
    let manifests = crate_manifests(&root, &files)?;

    run(Command::new(NIXFMT).current_dir(&root).args(&nix_files))?;
    run(Command::new(STATIX).arg("fix").arg(&root))?;
    for manifest in &manifests {
        run(Command::new(CARGO_FMT)
            .arg("--manifest-path")
            .arg(manifest)
            .env("CARGO", CARGO)
            .env("RUSTFMT", RUSTFMT))?;
    }
    Ok(())
}

fn repo_root() -> Result<PathBuf> {
    let out = output(Command::new(GIT).args(["rev-parse", "--show-toplevel"]))?;
    Ok(PathBuf::from(out.trim_end()))
}

/// Tracked and untracked files, gitignored excluded, relative to `root`.
fn repo_files(root: &Path) -> Result<Vec<String>> {
    let out = output(Command::new(GIT).current_dir(root).args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ]))?;
    Ok(out
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(String::from)
        .collect())
}

/// Every Cargo.toml that declares a package; virtual workspace roots are
/// skipped since their members are listed on their own.
fn crate_manifests(root: &Path, files: &[String]) -> Result<Vec<PathBuf>> {
    files
        .iter()
        .filter(|f| is_manifest(f))
        .map(|f| root.join(f))
        .filter_map(|path| match std::fs::read_to_string(&path) {
            Ok(text) if declares_package(&text) => Some(Ok(path)),
            Ok(_) => None,
            Err(e) => Some(Err(e).with_context(|| format!("reading {}", path.display()))),
        })
        .collect()
}

fn is_manifest(file: &str) -> bool {
    file == "Cargo.toml" || file.ends_with("/Cargo.toml")
}

fn declares_package(manifest: &str) -> bool {
    manifest.lines().any(|l| l.trim() == "[package]")
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed: {status}");
    }
    Ok(())
}

fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().with_context(|| format!("spawning {cmd:?}"))?;
    if !out.status.success() {
        bail!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim_end()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_match_only_cargo_toml() {
        assert!(is_manifest("Cargo.toml"));
        assert!(is_manifest("pkgs/seal/seal/Cargo.toml"));
        assert!(!is_manifest("pkgs/foo/NotCargo.toml"));
        assert!(!is_manifest("pkgs/foo/Cargo.lock"));
    }

    #[test]
    fn virtual_workspaces_declare_no_package() {
        assert!(declares_package("[package]\nname = \"x\"\n"));
        assert!(!declares_package("[workspace]\nmembers = [\"a\"]\n"));
    }
}
