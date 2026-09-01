//! Build the deterministic crew "quad" layout JSON for `cmux new-workspace --layout`.
//!
//! Geometry (fixed 2x2):
//! ```text
//! ┌──────────────────────────┬──────────────────────────┐
//! │ top-left pane            │ top-right pane           │
//! │   coordinator, planner   │   brainstorm             │
//! ├──────────────────────────┼──────────────────────────┤
//! │ bottom-left pane (devs)  │ bottom-right pane (devs) │
//! └──────────────────────────┴──────────────────────────┘
//! ```
//! Developers are assigned round-robin: dev 1,3,5… → bottom-left; 2,4,6… →
//! bottom-right. A quadrant that would be empty gets one bare terminal surface
//! so the quad is always fully provisioned.

use serde_json::{json, Value};

/// The inputs required to build a crew layout.
pub struct CrewSpec<'a> {
    /// Absolute state root (session dirs + fleet.md live under it).
    pub state_root: &'a str,
    /// Harness binary (`omp` | `claude` | `codex`).
    pub harness: &'a str,
    /// Number of disposable developers.
    pub devs: usize,
}

/// The harness launch command for a surface, honoring the session model:
/// masters are resumable (`--session-dir <role>`), developers ephemeral
/// (`--no-session`). Session isolation is `omp`-specific for now.
fn harness_command(spec: &CrewSpec, role: Option<&str>) -> String {
    match spec.harness {
        "omp" => match role {
            Some(r) => format!("omp --session-dir {}/sessions/{}", spec.state_root, r),
            None => "omp --no-session".to_string(),
        },
        other => other.to_string(), // claude/codex: launch bare; no session isolation yet
    }
}

fn surface(command: String) -> Value {
    json!({ "type": "terminal", "command": command })
}

/// Build the full `--layout` JSON value.
pub fn build(spec: &CrewSpec) -> Value {
    // Top panes.
    let top_left = json!({
        "pane": { "surfaces": [
            surface(harness_command(spec, Some("coordinator"))),
            surface(harness_command(spec, Some("planner"))),
        ]}
    });
    let top_right = json!({
        "pane": { "surfaces": [
            surface(harness_command(spec, Some("brainstorm"))),
        ]}
    });

    // Developer surfaces, round-robin across the two bottom quadrants.
    let mut bottom_left_surfaces: Vec<Value> = Vec::new();
    let mut bottom_right_surfaces: Vec<Value> = Vec::new();
    for k in 1..=spec.devs {
        let cmd = harness_command(spec, None);
        if k % 2 == 1 {
            bottom_left_surfaces.push(surface(cmd));
        } else {
            bottom_right_surfaces.push(surface(cmd));
        }
    }
    // Never leave an empty quadrant: a bare terminal as a slot.
    if bottom_left_surfaces.is_empty() {
        bottom_left_surfaces.push(json!({ "type": "terminal" }));
    }
    if bottom_right_surfaces.is_empty() {
        bottom_right_surfaces.push(json!({ "type": "terminal" }));
    }

    let bottom_left = json!({ "pane": { "surfaces": bottom_left_surfaces } });
    let bottom_right = json!({ "pane": { "surfaces": bottom_right_surfaces } });

    // Columns (vertical = stacked top/bottom), root (horizontal = side-by-side).
    let left =
        json!({ "direction": "vertical", "split": 0.5, "children": [top_left, bottom_left] });
    let right =
        json!({ "direction": "vertical", "split": 0.5, "children": [top_right, bottom_right] });

    json!({ "direction": "horizontal", "split": 0.5, "children": [left, right] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(devs: usize) -> CrewSpec<'static> {
        CrewSpec {
            state_root: "/state",
            harness: "omp",
            devs,
        }
    }

    fn leaf(v: &Value) -> &Vec<Value> {
        v["pane"]["surfaces"].as_array().unwrap()
    }

    #[test]
    fn quad_has_four_panes() {
        let layout = build(&spec(2));
        let left = &layout["children"][0];
        let right = &layout["children"][1];
        assert_eq!(layout["children"].as_array().unwrap().len(), 2);
        assert_eq!(leaf(&left["children"][0]).len(), 2); // coordinator + planner
        assert_eq!(leaf(&left["children"][1]).len(), 1); // dev-1
        assert_eq!(leaf(&right["children"][0]).len(), 1); // brainstorm
        assert_eq!(leaf(&right["children"][1]).len(), 1); // dev-2
    }

    #[test]
    fn odd_devs_round_robin_left_first() {
        let layout = build(&spec(3));
        let left_bottom = leaf(&layout["children"][0]["children"][1]);
        let right_bottom = leaf(&layout["children"][1]["children"][1]);
        assert_eq!(left_bottom.len(), 2); // dev-1, dev-3
        assert_eq!(right_bottom.len(), 1); // dev-2
    }

    #[test]
    fn zero_devs_still_fills_quad_with_slots() {
        let layout = build(&spec(0));
        assert_eq!(leaf(&layout["children"][0]["children"][1]).len(), 1);
        assert_eq!(leaf(&layout["children"][1]["children"][1]).len(), 1);
    }

    #[test]
    fn master_commands_carry_session_dir() {
        let layout = build(&spec(2));
        let coord = leaf(&layout["children"][0]["children"][0])[0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(coord, "omp --session-dir /state/sessions/coordinator");
        let dev1 = leaf(&layout["children"][0]["children"][1])[0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(dev1, "omp --no-session");
    }
}
