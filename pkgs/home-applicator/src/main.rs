use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use walkdir::WalkDir;

fn home_dir() -> PathBuf {
    dirs::home_dir().expect("HOME not set")
}

fn state_path() -> PathBuf {
    dirs::state_dir()
        .expect("XDG_STATE_HOME not set")
        .join("home-applicator/current")
}

fn relative_paths(root: &Path) -> Result<HashSet<PathBuf>> {
    let mut paths = HashSet::new();
    for entry in WalkDir::new(root).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_file() || entry.file_type().is_symlink() {
            paths.insert(entry.path().strip_prefix(root)?.to_owned());
        }
    }
    Ok(paths)
}

fn remove_old_symlinks(old_home_files: &Path) -> Result<()> {
    let home = home_dir();
    let paths = relative_paths(old_home_files)?;
    let mut removed = 0usize;
    for rel in &paths {
        let dest = home.join(rel);
        if let Ok(link_dest) = fs::read_link(&dest) {
            if link_dest == old_home_files.join(rel) {
                fs::remove_file(&dest)
                    .with_context(|| format!("removing {}", dest.display()))?;
                removed += 1;
            }
        }
    }
    eprintln!("removed {removed} old symlinks");
    Ok(())
}

fn create_new_symlinks(new_home_files: &Path) -> Result<()> {
    let home = home_dir();
    let paths = relative_paths(new_home_files)?;
    let total = paths.len();
    for rel in &paths {
        let src = new_home_files.join(rel);
        let dest = home.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating dir {}", parent.display()))?;
        }
        if dest.symlink_metadata().is_ok() {
            fs::remove_file(&dest)
                .with_context(|| format!("removing existing {}", dest.display()))?;
        }
        symlink(&src, &dest)
            .with_context(|| format!("symlinking {} -> {}", dest.display(), src.display()))?;
    }
    eprintln!("linked {total} files");
    Ok(())
}

fn systemd_units(home_files: &Path) -> HashSet<String> {
    let units_dir = home_files.join(".config/systemd/user");
    if !units_dir.exists() {
        return HashSet::new();
    }
    WalkDir::new(&units_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() || e.file_type().is_symlink())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_owned()))
        .collect()
}

fn unit_content(home_files: &Path, unit: &str) -> Option<Vec<u8>> {
    let path = home_files.join(".config/systemd/user").join(unit);
    fs::read(path).ok()
}

fn reconcile_systemd(old_home_files: Option<&Path>, new_home_files: &Path) -> Result<()> {
    let old_units: HashSet<String> = old_home_files.map(systemd_units).unwrap_or_default();
    let new_units = systemd_units(new_home_files);

    let changed: HashSet<&String> = old_units
        .iter()
        .filter(|u| {
            new_units.contains(*u)
                && unit_content(old_home_files.unwrap(), u) != unit_content(new_home_files, u)
        })
        .collect();

    let removed: HashSet<&String> = old_units.difference(&new_units).collect();
    let added: HashSet<&String> = new_units.difference(&old_units).collect();

    let to_stop: Vec<&&String> = removed.iter().chain(changed.iter()).collect();
    if !to_stop.is_empty() {
        eprintln!(
            "stopping: {}",
            to_stop.iter().map(|u| u.as_str()).collect::<Vec<_>>().join(", ")
        );
        Command::new("systemctl")
            .args(["--user", "stop"])
            .args(to_stop.iter().map(|u| u.as_str()))
            .status()
            .context("systemctl stop")?;
    }

    eprintln!("reloading systemd");
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("systemctl daemon-reload")?;

    let to_start: Vec<&&String> = added
        .iter()
        .chain(changed.iter())
        .filter(|u| u.ends_with(".service") || u.ends_with(".socket"))
        .collect();

    if !to_start.is_empty() {
        eprintln!(
            "starting: {}",
            to_start.iter().map(|u| u.as_str()).collect::<Vec<_>>().join(", ")
        );
        Command::new("systemctl")
            .args(["--user", "start"])
            .args(to_start.iter().map(|u| u.as_str()))
            .status()
            .context("systemctl start")?;
    }

    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let new_home_files = PathBuf::from(
        args.next()
            .context("usage: home-applicator <home-files-path> [home-path]")?,
    );

    let new_home_files = new_home_files
        .canonicalize()
        .with_context(|| format!("resolving {}", new_home_files.display()))?;

    eprintln!("applying {}", new_home_files.display());

    let state = state_path();
    let old_home_files: Option<PathBuf> = fs::read_to_string(&state)
        .ok()
        .map(|s| PathBuf::from(s.trim()));

    if let Some(ref old) = old_home_files {
        eprintln!("previous: {}", old.display());
        remove_old_symlinks(old)?;
    }

    create_new_symlinks(&new_home_files)?;

    reconcile_systemd(old_home_files.as_deref(), &new_home_files)?;

    if let Some(parent) = state.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&state, new_home_files.to_string_lossy().as_bytes())?;

    eprintln!("done");
    Ok(())
}
