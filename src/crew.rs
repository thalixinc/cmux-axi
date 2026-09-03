//! The crew spec (input) and the crew record (output).
//!
//! **Request** — what the caller asks for: `provision --spec <path|->` JSON. Flags override
//! its scalar fields. Seating is the caller's business (codefactory builds it per
//! project); with no `seats` the built-in default crew is used.
//!
//! **Record** — what `provision` actually built: `<state>/crews/<project>.json`. Read by
//! `dev add` (which slots take developers) and `status` (which layout); removed by
//! `teardown`. `fleet.md` stays the role → surface record; its grammar is parsed by
//! codefactory scripts and never changes.

use crate::error::{CmuxError, Result};
use crate::layout::Seating;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One requested seat. Only `role` is required.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SeatRequest {
    pub role: String,
    /// Absent: placed by the template's `default_seats`, then round-robin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
    /// Absent: `true` unless the role is `dev-N`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumable: Option<bool>,
    /// Tab title (default: from the role).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Full launch command, replacing the harness command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Default crew size; exclusive with `seats`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devs: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seats: Vec<SeatRequest>,
    /// Overrides the template's `dev_slots`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_slots: Option<Vec<usize>>,
}

impl Request {
    /// `--spec <path>` or `--spec -` (stdin).
    pub fn load(spec: &str) -> Result<Request> {
        let text = if spec == "-" {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).map_err(|e| {
                CmuxError::operational(format!("cannot read spec from stdin: {e}"), "SPEC_READ")
            })?;
            s
        } else {
            std::fs::read_to_string(spec).map_err(|e| {
                CmuxError::operational(format!("cannot read {spec}: {e}"), "SPEC_READ")
            })?
        };
        let req: Request = serde_json::from_str(&text)
            .map_err(|e| CmuxError::usage(format!("crew spec: {e}")))?;
        req.validate()?;
        Ok(req)
    }

    /// Structural checks that need no template: `devs` vs `seats`, roles present once.
    pub fn validate(&self) -> Result<()> {
        if self.devs.is_some() && !self.seats.is_empty() {
            return Err(CmuxError::usage("crew spec: give devs or seats, not both"));
        }
        let mut seen = std::collections::HashSet::new();
        for s in &self.seats {
            if s.role.trim().is_empty() {
                return Err(CmuxError::usage("crew spec: every seat needs a role"));
            }
            if !seen.insert(s.role.as_str()) {
                return Err(CmuxError::usage(format!("crew spec: role {:?} seated twice", s.role)));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub layout: String,
    pub slots: usize,
    pub dev_slots: Vec<usize>,
    /// Slot at each pane index (tree leaf order).
    pub leaf_order: Vec<usize>,
    pub tree: Value,
    pub seats: Seating,
    pub harness: String,
    pub cwd: String,
}

pub fn path(state: &Path, project: &str) -> PathBuf {
    state.join("crews").join(format!("{project}.json"))
}

/// `None` when the crew predates records (provisioned by cmux-axi ≤ 0.2.4).
pub fn load(state: &Path, project: &str) -> Result<Option<Record>> {
    let p = path(state, project);
    if !p.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).map_err(|e| {
        CmuxError::operational(format!("cannot read {}: {e}", p.display()), "CREW_READ")
    })?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| CmuxError::operational(format!("{}: invalid crew record: {e}", p.display()), "CREW_READ"))
}

pub fn write(state: &Path, project: &str, rec: &Record) -> Result<()> {
    let p = path(state, project);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CmuxError::operational(format!("cannot create {}: {e}", parent.display()), "CREW_WRITE")
        })?;
    }
    let body = serde_json::to_string_pretty(rec).unwrap_or_default();
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, body)
        .and_then(|_| std::fs::rename(&tmp, &p))
        .map_err(|e| CmuxError::operational(format!("cannot write {}: {e}", p.display()), "CREW_WRITE"))
}

pub fn remove(state: &Path, project: &str) {
    let _ = std::fs::remove_file(path(state, project));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Seat;

    #[test]
    fn request_rejects_unknown_fields_exclusive_devs_and_duplicate_roles() {
        assert!(serde_json::from_str::<Request>(r#"{"layout":"3by2","bogus":1}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"seats":[{"role":"x","nope":1}]}"#).is_err());
        let both: Request = serde_json::from_str(r#"{"devs":2,"seats":[{"role":"planner"}]}"#).unwrap();
        assert_eq!(both.validate().unwrap_err().exit_code(), 2);
        let dup: Request = serde_json::from_str(r#"{"seats":[{"role":"planner"},{"role":"planner"}]}"#).unwrap();
        assert!(dup.validate().unwrap_err().message.contains("seated twice"));
        let ok: Request = serde_json::from_str(r#"{"layout":"3by2","seats":[{"role":"planner","slot":1,"title":"P"}]}"#).unwrap();
        ok.validate().unwrap();
        assert_eq!(ok.seats[0].title.as_deref(), Some("P"));
    }

    #[test]
    fn record_round_trips_and_removes() {
        let dir = std::env::temp_dir().join(format!("cmux-axi-crew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load(&dir, "p").unwrap().is_none());
        let rec = Record {
            layout: "2by2".into(),
            slots: 4,
            dev_slots: vec![2, 3],
            leaf_order: vec![0, 1, 2, 3],
            tree: serde_json::json!({"direction":"vertical"}),
            seats: vec![Seat { role: "coordinator".into(), slot: 0, resumable: true, title: None, command: None }],
            harness: "omp".into(),
            cwd: "/tmp".into(),
        };
        write(&dir, "p", &rec).unwrap();
        assert_eq!(load(&dir, "p").unwrap().unwrap(), rec);
        remove(&dir, "p");
        assert!(load(&dir, "p").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
