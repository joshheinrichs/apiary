use std::os::unix::process::CommandExt;
use std::process::{self, Command};

use anyhow::{Context, Result};
use clap::Parser;
use procfs::process::Process;

const SYSTEMD_RUN: &str = match option_env!("SYSTEMD_RUN") {
    Some(s) => s,
    None => "systemd-run",
};

#[derive(Parser)]
struct Args {
    /// Slice name to create/use, relative to the detected parent slice
    #[arg(long)]
    slice: String,

    /// Scope unit name (defaults to app-<name>-<pid>)
    #[arg(long)]
    name: Option<String>,

    /// Command to run
    #[arg(trailing_var_arg = true, required = true)]
    cmd: Vec<String>,
}

fn unit_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c.to_string()
            } else {
                "-".to_string()
            }
        })
        .collect()
}

// Systemd slice unit names use `-` as the hierarchy separator, not `/`.
// e.g., parent="session-3.slice", child="apps" -> "session-3-apps.slice"
fn build_target_slice(parent: &str, child: &str) -> String {
    let child_base = child.trim_end_matches(".slice");
    if parent.is_empty() {
        return format!("{child_base}.slice");
    }
    let parent_base = parent
        .split('/')
        .map(|p| p.trim_end_matches(".slice"))
        .collect::<Vec<_>>()
        .join("-");
    format!("{parent_base}-{child_base}.slice")
}

fn parent_slice_from_cgroup(path: &str) -> &str {
    let path = path.trim_start_matches('/');

    // Strip user@NNN.service/ prefix to get user-manager-relative path
    let user_rel = path
        .split('/')
        .enumerate()
        .find(|(_, part)| part.starts_with("user@") && part.ends_with(".service"))
        .map(|(i, _)| {
            let prefix_len: usize = path.split('/').take(i + 1).map(|s| s.len() + 1).sum();
            &path[prefix_len.min(path.len())..]
        })
        .unwrap_or("");

    // Take the immediate parent unit (second-to-last component).
    // Systemd unit names encode the full hierarchy themselves, so the immediate
    // parent unit name is sufficient — using the full cgroup path would
    // cause duplicate prefix segments (e.g. session-session-1-apps.slice).
    user_rel.rsplit('/').nth(1).unwrap_or("")
}

fn detect_parent_slice() -> Result<String> {
    let cgroups = Process::myself()
        .context("failed to open /proc/self")?
        .cgroups()
        .context("failed to read cgroups")?;

    let path = cgroups
        .0
        .into_iter()
        .find(|e| e.hierarchy == 0)
        .map(|e| e.pathname)
        .unwrap_or_default();

    Ok(parent_slice_from_cgroup(&path).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_target_slice_no_parent() {
        assert_eq!(build_target_slice("", "session-1"), "session-1.slice");
    }

    #[test]
    fn build_target_slice_no_parent_with_suffix() {
        assert_eq!(build_target_slice("", "session-1.slice"), "session-1.slice");
    }

    #[test]
    fn build_target_slice_with_parent() {
        assert_eq!(build_target_slice("session-1.slice", "apps"), "session-1-apps.slice");
    }

    #[test]
    fn build_target_slice_with_parent_with_suffix() {
        assert_eq!(build_target_slice("session-1.slice", "apps.slice"), "session-1-apps.slice");
    }

    #[test]
    fn parent_slice_from_cgroup_inside_user_manager() {
        let path = "/user.slice/user-1000.slice/user@1000.service/session.slice/session-1.slice/sway.scope";
        assert_eq!(parent_slice_from_cgroup(path), "session-1.slice");
    }

    #[test]
    fn parent_slice_from_cgroup_inside_apps_slice() {
        let path = "/user.slice/user-1000.slice/user@1000.service/session.slice/session-1.slice/session-1-apps.slice/app-discord-123.scope";
        assert_eq!(parent_slice_from_cgroup(path), "session-1-apps.slice");
    }

    #[test]
    fn parent_slice_from_cgroup_outside_user_manager() {
        // TTY login — no user@NNN.service in path
        let path = "/user.slice/user-1000.slice/session-1.scope";
        assert_eq!(parent_slice_from_cgroup(path), "");
    }

    #[test]
    fn unit_escape_clean() {
        assert_eq!(unit_escape("discord"), "discord");
    }

    #[test]
    fn unit_escape_special_chars() {
        assert_eq!(unit_escape("my/app name"), "my-app-name");
    }

    #[test]
    fn slice_base_prefixes_named_scope() {
        let slice_base = "session-1-apps";
        let name = "discord";
        assert_eq!(format!("{slice_base}-{name}.scope"), "session-1-apps-discord.scope");
    }

    #[test]
    fn slice_base_prefixes_sway_scope() {
        let slice_base = "session-1";
        let name = "sway";
        assert_eq!(format!("{slice_base}-{name}.scope"), "session-1-sway.scope");
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let parent = detect_parent_slice()?;
    let target_slice = build_target_slice(&parent, &args.slice);

    let slice_base = target_slice.trim_end_matches(".slice");
    let unit_name = match args.name {
        Some(ref n) => format!("{slice_base}-{}.scope", n.trim_end_matches(".scope")),
        None => {
            let basename = args.cmd[0].split('/').last().unwrap_or(&args.cmd[0]);
            format!("{slice_base}-app-{}-{}.scope", unit_escape(basename), process::id())
        }
    };

    Err(Command::new(SYSTEMD_RUN)
        .args([
            "--user",
            "--scope",
            &format!("--slice={target_slice}"),
            &format!("--unit={unit_name}"),
            "--",
        ])
        .args(&args.cmd)
        .exec()
        .into())
}
