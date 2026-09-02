//! Version + self-update for a `cargo install --git`-distributed binary.
//!
//! `version` prints the version. `update --check` compares against the version
//! on the default branch; `update` reinstalls latest from the repo. Version
//! identity lives in `Cargo.toml` (single source of truth).

use crate::error::{CmuxError, Result};
use crate::toon;
use std::cmp::Ordering;
use std::process::Command;

pub const REPO: &str = "https://github.com/thalixinc/cmux-axi";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn cmd_version() {
    println!("cmux-axi {VERSION}");
}

/// Compare dotted version strings element-wise ("0.2.0" vs "0.10.1").
fn semver_cmp(a: &str, b: &str) -> Ordering {
    let pa: Vec<u64> = a.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    for i in 0..pa.len().max(pb.len()) {
        match pa
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&pb.get(i).copied().unwrap_or(0))
        {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// Fetch the version string from the default branch's `Cargo.toml`.
fn fetch_latest_version() -> Result<String> {
    let url = "https://raw.githubusercontent.com/thalixinc/cmux-axi/main/Cargo.toml";
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "10", url])
        .output()
        .map_err(|e| {
            CmuxError::operational(format!("`curl` not available: {e}"), "UPDATE_CHECK")
        })?;
    if !out.status.success() {
        return Err(CmuxError::operational(
            "could not reach the version source (network or curl failure)",
            "UPDATE_CHECK",
        )
        .with_suggestions(vec![
            "Check network access and that `curl` is installed.".into()
        ]));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("version = \"")
                .and_then(|r| r.strip_suffix('"'))
                .map(String::from)
        })
        .ok_or_else(|| {
            CmuxError::operational("version not found in remote Cargo.toml", "UPDATE_CHECK")
        })
}

/// `cmux-axi update [--check]`.
pub fn cmd_update(check: bool, json: bool) -> Result<()> {
    let latest = fetch_latest_version()?;
    let available = semver_cmp(&latest, VERSION) == Ordering::Greater;

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "package": "cmux-axi", "current": VERSION, "latest": latest, "available": available,
            }))
            .unwrap()
        );
    } else {
        println!(
            "{}",
            toon::join(&[
                format!("update:\n  package: cmux-axi\n  current: {VERSION}\n  latest: {latest}\n  available: {available}"),
                if available {
                    toon::help(&["Run `cmux-axi update` to upgrade".to_string()])
                } else {
                    toon::help(&["Already up to date".to_string()])
                },
            ])
        );
    }

    if check {
        return Ok(());
    }

    // Actually update: reinstall latest from the repo.
    let status = Command::new("cargo")
        .args(["install", "--git", REPO, "--force"])
        .status()
        .map_err(|e| CmuxError::operational(format!("`cargo` not available: {e}"), "UPDATE"))?;
    if !status.success() {
        return Err(
            CmuxError::operational("cargo install failed", "UPDATE").with_suggestions(vec![
                format!("Run `cargo install --git {REPO} --force` manually."),
            ]),
        );
    }
    if !available {
        println!("update: cmux-axi already at latest ({VERSION})");
        return Ok(());
    }
    println!("update: cmux-axi upgraded {VERSION} -> {latest}");
    Ok(())
}
