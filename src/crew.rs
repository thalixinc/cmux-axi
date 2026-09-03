//! The crew record — what `provision` actually built for a project:
//! `<state>/crews/<project>.json`. Read by `dev add` (which slots take developers) and
//! `status` (which layout); removed by `teardown`. `fleet.md` stays the role → surface
//! record; its grammar is parsed by codefactory scripts and never changes.

use crate::error::{CmuxError, Result};
use crate::layout::Seating;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

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
            seats: vec![Seat { role: "coordinator".into(), slot: 0, resumable: true }],
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
