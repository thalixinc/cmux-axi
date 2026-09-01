//! The `fleet.md` record — the durable role → surface → session binding.
//!
//! Line grammar (compatible with PLAN-CHIEF-OF-STAFF §3):
//! `- <role> - <project> (surface: <ref>; session: <id>; status: <status>; started <date>)`
//!
//! `session` is a session id for resumable roles, or `ephemeral` for disposable
//! developers. cmux-axi is the single writer; COF/agents read it (never a second
//! source of truth).

use crate::error::{CmuxError, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct FleetEntry {
    pub role: String,
    pub project: String,
    pub surface: String,
    pub session: String,
    pub status: String,
    pub started: String,
}

impl FleetEntry {
    fn to_line(&self) -> String {
        format!(
            "- {} - {} (surface: {}; session: {}; status: {}; started {})",
            self.role, self.project, self.surface, self.session, self.status, self.started
        )
    }

    /// Parse one fleet line. Returns `None` for blank/header/unrelated lines.
    fn from_line(line: &str) -> Option<FleetEntry> {
        let line = line.trim();
        let rest = line.strip_prefix("- ")?;
        let (role, rest) = rest.split_once(" - ")?;
        let (project, parens) = rest.split_once(" (")?;
        let parens = parens.strip_suffix(')')?;

        let mut surface = None;
        let mut session = None;
        let mut status = None;
        let mut started = None;
        for part in parens.split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix("surface: ") {
                surface = Some(v.to_string());
            } else if let Some(v) = part.strip_prefix("session: ") {
                session = Some(v.to_string());
            } else if let Some(v) = part.strip_prefix("status: ") {
                status = Some(v.to_string());
            } else if let Some(v) = part.strip_prefix("started ") {
                started = Some(v.to_string());
            }
        }

        Some(FleetEntry {
            role: role.trim().to_string(),
            project: project.trim().to_string(),
            surface: surface?,
            session: session.unwrap_or_default(),
            status: status.unwrap_or_else(|| "unknown".to_string()),
            started: started.unwrap_or_default(),
        })
    }
}

/// Load all fleet entries from a file (missing file → empty list).
pub fn load(path: &Path) -> Result<Vec<FleetEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|e| {
        CmuxError::operational(format!("cannot read {}: {e}", path.display()), "FLEET_READ")
    })?;
    Ok(text.lines().filter_map(FleetEntry::from_line).collect())
}

/// Write entries atomically (temp file + rename).
pub fn write(path: &Path, entries: &[FleetEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            CmuxError::operational(
                format!("cannot create {}: {e}", parent.display()),
                "FLEET_WRITE",
            )
        })?;
    }
    let mut body = String::from("# cmux-axi fleet — role → surface → session\n");
    for e in entries {
        body.push_str(&e.to_line());
        body.push('\n');
    }
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, &body).map_err(|e| {
        CmuxError::operational(
            format!("cannot write {}: {e}", tmp.display()),
            "FLEET_WRITE",
        )
    })?;
    fs::rename(&tmp, path).map_err(|e| {
        CmuxError::operational(
            format!("cannot rename to {}: {e}", path.display()),
            "FLEET_WRITE",
        )
    })?;
    Ok(())
}

/// Upsert `entry` into the loaded list, matching on (project, role).
pub fn upsert(entries: &mut Vec<FleetEntry>, entry: FleetEntry) {
    if let Some(existing) = entries
        .iter_mut()
        .find(|e| e.project == entry.project && e.role == entry.role)
    {
        *existing = entry;
    } else {
        entries.push(entry);
    }
}

/// Remove the entry matching (project, role).
pub fn remove(entries: &mut Vec<FleetEntry>, project: &str, role: &str) {
    entries.retain(|e| !(e.project == project && e.role == role));
}

/// Find an entry by (project, role).
pub fn find<'a>(entries: &'a [FleetEntry], project: &str, role: &str) -> Option<&'a FleetEntry> {
    entries
        .iter()
        .find(|e| e.project == project && e.role == role)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: &str, surface: &str) -> FleetEntry {
        FleetEntry {
            role: role.into(),
            project: "proj".into(),
            surface: surface.into(),
            session: "ephemeral".into(),
            status: "active".into(),
            started: "2026-09-01".into(),
        }
    }

    #[test]
    fn line_round_trips() {
        let e = entry("planner", "surface:5");
        let parsed = FleetEntry::from_line(&e.to_line()).unwrap();
        assert_eq!(parsed, e);
    }

    #[test]
    fn ignores_non_fleet_lines() {
        assert!(FleetEntry::from_line("# header").is_none());
        assert!(FleetEntry::from_line("").is_none());
        assert!(FleetEntry::from_line("just text").is_none());
    }

    #[test]
    fn upsert_replaces_same_role() {
        let mut v = vec![entry("planner", "surface:5")];
        upsert(&mut v, entry("planner", "surface:9"));
        assert_eq!(v.len(), 1);
        assert_eq!(find(&v, "proj", "planner").unwrap().surface, "surface:9");
    }

    #[test]
    fn remove_drops_only_match() {
        let mut v = vec![entry("planner", "surface:5"), entry("dev-1", "surface:6")];
        remove(&mut v, "proj", "planner");
        assert_eq!(v.len(), 1);
        assert!(find(&v, "proj", "planner").is_none());
        assert!(find(&v, "proj", "dev-1").is_some());
    }
}
