use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use walkdir::WalkDir;

mod watch;

fn home_dir() -> PathBuf {
    dirs::home_dir().expect("HOME not set")
}

const NIX_STORE: &str = env!("NIX_STORE_BIN");

fn state_path() -> PathBuf {
    dirs::state_dir()
        .expect("XDG_STATE_HOME not set")
        .join("home-applicator/current")
}

/// Keep the applied home and its closure alive: an indirect GC root beside the state file.
fn add_gc_root(home_files: &Path) -> Result<()> {
    let root = state_path().with_file_name("gcroot");
    let status = Command::new(NIX_STORE)
        .arg("--add-root")
        .arg(&root)
        .arg("--realise")
        .arg(home_files)
        .stdout(std::process::Stdio::null())
        .status()
        .context("running nix-store --add-root")?;
    anyhow::ensure!(
        status.success(),
        "nix-store --add-root {} failed",
        root.display()
    );
    Ok(())
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

fn remove_old_symlinks(
    old_home_files: &Path,
    paths: &HashSet<PathBuf>,
    progress: &mut dyn FnMut(usize),
) -> Result<usize> {
    let home = home_dir();
    let mut removed = 0usize;
    for (i, rel) in paths.iter().enumerate() {
        progress(i);
        let dest = home.join(rel);
        if fs::read_link(&dest).is_ok_and(|l| l == old_home_files.join(rel)) {
            fs::remove_file(&dest).with_context(|| format!("removing {}", dest.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn create_new_symlinks(
    new_home_files: &Path,
    paths: &HashSet<PathBuf>,
    progress: &mut dyn FnMut(usize),
) -> Result<usize> {
    let home = home_dir();
    for (i, rel) in paths.iter().enumerate() {
        progress(i);
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
    Ok(paths.len())
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

fn plan_systemd(old_home_files: Option<&Path>, new_home_files: &Path) -> watch::Plan {
    let old_units: HashSet<String> = old_home_files.map(systemd_units).unwrap_or_default();
    let new_units = systemd_units(new_home_files);

    let sorted = |units: Vec<&String>| {
        let mut units: Vec<String> = units.into_iter().cloned().collect();
        units.sort();
        units
    };
    let changed = old_units
        .iter()
        .filter(|u| {
            new_units.contains(*u)
                && unit_content(old_home_files.unwrap(), u) != unit_content(new_home_files, u)
        })
        .collect();
    watch::Plan {
        removed: sorted(old_units.difference(&new_units).collect()),
        changed: sorted(changed),
        added: sorted(new_units.difference(&old_units).collect()),
    }
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

    let state = state_path();
    let old_home_files: Option<PathBuf> = fs::read_to_string(&state)
        .ok()
        .map(|s| PathBuf::from(s.trim()));
    // Collected before it was rooted: its links can no longer be told apart, so the new ones
    // just replace whatever is at their paths.
    let old_paths = match &old_home_files {
        Some(old) if old.exists() => relative_paths(old)?,
        _ => HashSet::new(),
    };
    let new_paths = relative_paths(&new_home_files)?;
    let plan = plan_systemd(old_home_files.as_deref(), &new_home_files);

    let goal = new_home_files.display().to_string();
    let mut session = watch::Session::open(goal, &plan)?;
    session.step(
        watch::Verb::Unlink,
        old_paths.len(),
        |progress| match &old_home_files {
            Some(old) => remove_old_symlinks(old, &old_paths, progress),
            None => Ok(0),
        },
    )?;
    session.step(watch::Verb::Link, new_paths.len(), |progress| {
        create_new_symlinks(&new_home_files, &new_paths, progress)
    })?;
    let failed = session.systemd(&plan)?;
    drop(session);

    if let Some(parent) = state.parent() {
        fs::create_dir_all(parent)?;
    }
    add_gc_root(&new_home_files)?;
    fs::write(&state, new_home_files.to_string_lossy().as_bytes())?;

    anyhow::ensure!(failed.is_empty(), "failed to start: {}", failed.join(", "));
    Ok(())
}
