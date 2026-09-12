//! Version + self-update for a `cargo install --git`-distributed binary.
//!
//! `version` prints the version. `update --check` compares against the version
//! on the default branch; `update` reinstalls only when a newer version exists.
//! Version identity lives in `Cargo.toml` (single source of truth).

use crate::error::{CmuxError, Result};
use crate::toon;
use std::cmp::Ordering;
use std::io::IsTerminal;
use std::process::Command;

pub const REPO: &str = "https://github.com/thalixinc/cmux-axi";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn cmd_version(yes: bool) -> Result<()> {
    // Always print the version line first (script-safe; `-v`/`-V`/`--version` stop here).
    println!("cmux-axi {VERSION}");

    // The update-available surface (mirrors cf #368): a newer release → "update available";
    // `--yes` auto-updates; otherwise an interactive [y/N] (non-tty = reported, never blocks).
    // An offline/network fetch failure is not fatal: the version line is already printed.
    version_update_surface(yes, fetch_latest_version())
}

/// The update-available logic once the version line is printed and the latest
/// version is resolved. Split out so the comparison guard and the non-fatal
/// offline behaviour are unit-testable without hitting the network.
fn version_update_surface(yes: bool, fetched: Result<String>) -> Result<()> {
    let latest = match fetched {
        Ok(v) => v,
        Err(_) => return Ok(()), // offline: just the version line, exit 0.
    };
    if semver_cmp(&latest, VERSION) != Ordering::Greater {
        return Ok(()); // on latest: just the version line, no prompt.
    }

    if yes {
        do_update()?;
        println!("update: cmux-axi upgraded {VERSION} -> {latest}");
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        println!("update available: {latest} — run `cmux-axi update`, or `cmux-axi version --yes` to update now");
        return Ok(());
    }
    print!("update available: {latest} — run cmux-axi update, or --yes to update now [y/N] ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if matches!(line.trim(), "y" | "Y" | "yes" | "YES") {
        do_update()?;
        println!("update: cmux-axi upgraded {VERSION} -> {latest}");
    }
    // n / anything else / empty: exit 0, no change.
    Ok(())
}

/// The `cargo install --git <REPO> --force` update, shared by `cmux-axi update` and
/// `cmux-axi version --yes`.
fn do_update() -> Result<()> {
    let status = Command::new("cargo")
        .args(["install", "--git", REPO, "--force"])
        .status()
        .map_err(|e| CmuxError::operational(format!("`cargo` not available: {e}"), "UPDATE"))?;
    if !status.success() {
        return Err(CmuxError::operational("cargo install failed", "UPDATE").with_suggestions(vec![
            format!("Run `cargo install --git {REPO} --force` manually."),
        ]));
    }
    Ok(())
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

/// Fetch the version string from the default branch's `Cargo.toml` via the
/// GitHub API (authoritative — the raw CDN lags for minutes after a push).
fn fetch_latest_version() -> Result<String> {
    let url = "https://api.github.com/repos/thalixinc/cmux-axi/contents/Cargo.toml";
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github.raw+json",
            url,
        ])
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

    // `update --check`: report current vs latest, install nothing.
    if check {
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
                    format!(
                        "update:\n  package: cmux-axi\n  current: {VERSION}\n  latest: {latest}\n  available: {available}"
                    ),
                    if available {
                        toon::help(&["Run `cmux-axi update` to upgrade".to_string()])
                    } else {
                        toon::help(&["Already up to date".to_string()])
                    },
                ])
            );
        }
        return Ok(());
    }

    // `update`: reinstall only if a newer version exists.
    if !available {
        if json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "ok": true, "action": "update", "current": VERSION, "latest": latest, "available": false,
                }))
                .unwrap()
            );
        } else {
            println!("ok: cmux-axi already at latest ({VERSION})");
        }
        return Ok(());
    }

    do_update()?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": true, "action": "update", "current": VERSION, "latest": latest, "available": true,
            }))
            .unwrap()
        );
    } else {
        println!("update: cmux-axi upgraded {VERSION} -> {latest}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_fetch_is_not_fatal() {
        // A network/curl failure must not propagate out of `version`: the version
        // line is already printed, so the command degrades to exit 0.
        let err = CmuxError::operational("could not reach the version source", "UPDATE_CHECK");
        assert!(version_update_surface(false, Err(err)).is_ok());
    }

    #[test]
    fn no_update_surface_when_not_greater() {
        // latest == current, and latest older than current: just Ok(()), no prompt.
        assert!(version_update_surface(false, Ok(VERSION.to_string())).is_ok());
        assert!(version_update_surface(false, Ok("0.0.1".to_string())).is_ok());
    }
}
