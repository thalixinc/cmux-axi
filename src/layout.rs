//! Seating + the cmux `--layout` tree for a crew.
//!
//! The geometry comes from a `templates::Template` (structure only: slots in spatial
//! order, `dev_slots`, `default_seats`). This module decides *who sits where*
//! (`Seating`) and renders the tree that `provision` hands to `cmux new-workspace`.
//!
//! Default crew (no crew spec): masters `coordinator`, `planner`, `brainstorm` take the
//! template's `default_seats`, or round-robin over the non-dev slots; `dev-k` goes to
//! `dev_slots[(k-1) % len]`. Several seats in one slot are tabs, in seat order. A slot
//! nobody sits in gets one bare terminal so the pane exists.

use crate::error::{CmuxError, Result};
use crate::templates::{self, Template};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// The inputs required to launch a harness in a pane.
pub struct CrewSpec<'a> {
    /// Absolute state root (session dirs + fleet.md live under it).
    pub state_root: &'a str,
    /// Harness binary (`omp` | `claude` | `codex`).
    pub harness: &'a str,
    /// Project the crew works on (exported to every pane as `CF_PROJECT`).
    pub project: &'a str,
    /// The Chief-of-Staff home (state root's grandparent; exported as `CF_COF_HOME`).
    /// When it holds codefactory's session-start extension / crew config, panes load them.
    pub cof_home: &'a str,
}

/// One agent in one slot. Persisted verbatim in the crew record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Seat {
    pub role: String,
    pub slot: usize,
    pub resumable: bool,
}

pub type Seating = Vec<Seat>;

pub const MASTERS: [&str; 3] = ["coordinator", "planner", "brainstorm"];

/// Today's crew, seated by the template's defaults.
pub fn default_seating(t: &Template, devs: usize) -> Result<Seating> {
    let n = templates::slots(t);
    let non_dev: Vec<usize> = (0..n).filter(|s| !t.dev_slots.contains(s)).collect();
    let pool = if non_dev.is_empty() { (0..n).collect::<Vec<_>>() } else { non_dev };
    let mut seats = Seating::new();
    let mut rr = 0;
    for role in MASTERS {
        let slot = match t.default_seats.get(role) {
            Some(s) => *s,
            None => {
                let s = pool[rr % pool.len()];
                rr += 1;
                s
            }
        };
        seats.push(Seat { role: role.to_string(), slot, resumable: true });
    }
    if devs > 0 && t.dev_slots.is_empty() {
        return Err(CmuxError::usage(format!(
            "layout {:?} has no dev_slots; use --devs 0 or another layout",
            t.name
        )));
    }
    for k in 1..=devs {
        seats.push(Seat {
            role: format!("dev-{k}"),
            slot: t.dev_slots[(k - 1) % t.dev_slots.len()],
            resumable: false,
        });
    }
    Ok(seats)
}

/// Single-quote a string for the pane's shell command line.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The harness launch command for a surface, honoring the session model:
/// masters are resumable (`--session-dir <role>`), developers ephemeral
/// (`--no-session`). Every `omp` pane is told who it is (`CF_ROLE`, `CF_PROJECT`,
/// `CF_COF_HOME`) and, when the Chief-of-Staff home provides them, loads codefactory's
/// session-start extension (`-e`) and crew config overlay (`--config`) — that is what
/// makes a pane open as its role instead of as a bare shell. Session isolation is
/// `omp`-specific for now.
pub fn harness_command(spec: &CrewSpec, role: &str, resumable: bool) -> String {
    match spec.harness {
        "omp" => {
            let mut cmd = format!(
                "CF_ROLE={} CF_PROJECT={} CF_COF_HOME={} omp",
                sh_quote(role),
                sh_quote(spec.project),
                sh_quote(spec.cof_home)
            );
            let ext = Path::new(spec.cof_home).join(".omp/extensions/cf-session-start.ts");
            if ext.exists() {
                cmd.push_str(&format!(" -e {}", sh_quote(&ext.to_string_lossy())));
            }
            let cfg = Path::new(spec.cof_home).join(".omp/crew-config.yml");
            if cfg.exists() {
                cmd.push_str(&format!(" --config {}", sh_quote(&cfg.to_string_lossy())));
            }
            if resumable {
                cmd.push_str(&format!(
                    " --session-dir {}",
                    sh_quote(&format!("{}/sessions/{}", spec.state_root, role))
                ));
            } else {
                cmd.push_str(" --no-session");
            }
            cmd
        }
        other => other.to_string(), // claude/codex: launch bare; no session isolation yet
    }
}

fn surface(command: String) -> Value {
    json!({ "type": "terminal", "command": command })
}

/// Build the full `--layout` JSON value: slot n holds the surfaces seated there.
pub fn build(spec: &CrewSpec, t: &Template, seating: &Seating) -> Value {
    let n = templates::slots(t);
    let leaves: Vec<Value> = (0..n)
        .map(|slot| {
            let surfaces: Vec<Value> = seating
                .iter()
                .filter(|s| s.slot == slot)
                .map(|s| surface(harness_command(spec, &s.role, s.resumable)))
                .collect();
            if surfaces.is_empty() {
                templates::bare()
            } else {
                json!({ "pane": { "surfaces": surfaces } })
            }
        })
        .collect();
    templates::compile(t, leaves)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> CrewSpec<'static> {
        CrewSpec {
            state_root: "/state",
            harness: "omp",
            project: "demo",
            cof_home: "/nonexistent-home",
        }
    }

    fn t() -> Template {
        templates::resolve("2by2").unwrap()
    }

    /// Leaf panes' surface arrays in tree (= slot) order.
    fn leaves(v: &Value, out: &mut Vec<Vec<Value>>) {
        if let Some(p) = v.get("pane") {
            out.push(p["surfaces"].as_array().unwrap().clone());
            return;
        }
        for c in v["children"].as_array().unwrap() {
            leaves(c, out);
        }
    }

    fn built(devs: usize) -> Vec<Vec<Value>> {
        let seating = default_seating(&t(), devs).unwrap();
        let mut out = Vec::new();
        leaves(&build(&spec(), &t(), &seating), &mut out);
        out
    }

    #[test]
    fn quad_has_four_panes() {
        let l = built(2);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0].len(), 2); // coordinator + planner
        assert_eq!(l[1].len(), 1); // brainstorm
        assert_eq!(l[2].len(), 1); // dev-1
        assert_eq!(l[3].len(), 1); // dev-2
    }

    #[test]
    fn odd_devs_round_robin_left_first() {
        let l = built(3);
        assert_eq!(l[2].len(), 2); // dev-1, dev-3
        assert_eq!(l[3].len(), 1); // dev-2
    }

    #[test]
    fn zero_devs_still_fills_quad_with_slots() {
        let l = built(0);
        assert_eq!(l[2].len(), 1);
        assert_eq!(l[3].len(), 1);
        assert!(l[2][0].get("command").is_none()); // bare terminal
    }

    #[test]
    fn master_commands_carry_session_dir() {
        let l = built(2);
        let coord = l[0][0]["command"].as_str().unwrap();
        assert_eq!(
            coord,
            "CF_ROLE='coordinator' CF_PROJECT='demo' CF_COF_HOME='/nonexistent-home' omp --session-dir '/state/sessions/coordinator'"
        );
        let dev1 = l[2][0]["command"].as_str().unwrap();
        assert_eq!(dev1, "CF_ROLE='dev-1' CF_PROJECT='demo' CF_COF_HOME='/nonexistent-home' omp --no-session");
    }

    #[test]
    fn default_seating_round_robins_masters_without_default_seats() {
        let mut tpl = t();
        tpl.default_seats.clear();
        let s = default_seating(&tpl, 1).unwrap();
        let slots: Vec<usize> = s.iter().map(|x| x.slot).collect();
        assert_eq!(slots, vec![0, 1, 0, 2]); // coordinator, planner, brainstorm over slots 0,1; dev-1 → 2
    }

    #[test]
    fn devs_need_dev_slots() {
        let mut tpl = t();
        tpl.dev_slots.clear();
        assert!(default_seating(&tpl, 0).is_ok());
        assert_eq!(default_seating(&tpl, 1).unwrap_err().exit_code(), 2);
    }

    #[test]
    fn panes_load_codefactory_extension_and_crew_config_when_the_home_has_them() {
        let home = std::env::temp_dir().join(format!("cmux-axi-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".omp/extensions")).unwrap();
        std::fs::write(home.join(".omp/extensions/cf-session-start.ts"), "// ext").unwrap();
        std::fs::write(home.join(".omp/crew-config.yml"), "features: {}").unwrap();
        let home_s = home.to_string_lossy().to_string();
        let spec = CrewSpec { state_root: "/state", harness: "omp", project: "it's demo", cof_home: &home_s };
        let cmd = harness_command(&spec, "planner", true);
        assert!(cmd.contains(&format!(" -e '{}/.omp/extensions/cf-session-start.ts'", home_s)), "{cmd}");
        assert!(cmd.contains(&format!(" --config '{}/.omp/crew-config.yml'", home_s)), "{cmd}");
        assert!(cmd.starts_with("CF_ROLE='planner' CF_PROJECT='it'\\''s demo' "), "{cmd}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
