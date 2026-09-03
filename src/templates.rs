//! Layout templates — the crew's *structure only*, as JSON data.
//!
//! A template is rows of pane counts (or a raw cmux split tree for odd shapes). Its
//! leaf panes are **slots**, numbered in spatial order: top row left→right, then the
//! next row. Who sits in which slot is the crew's business (`layout::Seating`); a
//! template only carries `dev_slots` (where developer tabs go) and optional
//! `default_seats` (role → slot) so the tool works with no crew spec.
//!
//! Built-ins are the same JSON as user files (`~/.config/cmux-axi/layouts/<name>.json`).
//!
//! Measured 2026-09-03 against cmux: `split` is the first child's share; panes in a row
//! share an exact `y`; and an **unfocused** workspace reports all-zero pixel frames, so
//! panes are matched to slots by pane index = tree leaf order (see `leaf_order`), never
//! by geometry.

use crate::error::{CmuxError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The template that `provision` uses when no `--layout` is given.
pub const DEFAULT: &str = "3by2";

const BUILTIN_JSON: &[&str] = &[
    include_str!("../layouts/2by2.json"),
    include_str!("../layouts/3by2.json"),
];

/// One row of a `rows` template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub panes: usize,
    /// Fraction of the workspace height (all rows or none; default equal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Fraction of the row width per pane (default equal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widths: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Row>>,
    /// Escape hatch: a cmux `--layout` tree whose leaves are `{"slot": n}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<Value>,
    /// Slots that receive developer tabs (round-robin).
    #[serde(default)]
    pub dev_slots: Vec<usize>,
    /// role → slot used when the caller supplies no seating.
    #[serde(default)]
    pub default_seats: BTreeMap<String, usize>,
}

/// Where a template came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Builtin,
    User(PathBuf),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Builtin => write!(f, "built-in"),
            Source::User(p) => write!(f, "{}", p.display()),
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

pub fn builtins() -> Vec<Template> {
    BUILTIN_JSON
        .iter()
        .map(|s| serde_json::from_str(s).expect("built-in layout JSON is valid"))
        .collect()
}

/// `$XDG_CONFIG_HOME/cmux-axi/layouts`, default `~/.config/cmux-axi/layouts`.
pub fn user_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("cmux-axi").join("layouts");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("cmux-axi").join("layouts")
}

pub fn load_file(path: &Path) -> Result<Template> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        CmuxError::operational(format!("cannot read {}: {e}", path.display()), "LAYOUT_READ")
    })?;
    let t: Template = serde_json::from_str(&text)
        .map_err(|e| CmuxError::usage(format!("{}: invalid layout template: {e}", path.display())))?;
    validate(&t)?;
    Ok(t)
}

/// Built-ins first, then user files (sorted by name). A user file that fails to parse is
/// skipped with a warning on stderr, never fatal.
pub fn list() -> Vec<(Template, Source)> {
    let mut out: Vec<(Template, Source)> =
        builtins().into_iter().map(|t| (t, Source::Builtin)).collect();
    if let Ok(rd) = std::fs::read_dir(user_dir()) {
        let mut paths: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        paths.sort();
        for p in paths {
            match load_file(&p) {
                Ok(t) => out.push((t, Source::User(p))),
                Err(e) => eprintln!("cmux-axi: skipping {}: {}", p.display(), e.message),
            }
        }
    }
    out
}

pub fn known_names() -> Vec<String> {
    list().into_iter().map(|(t, _)| t.name).collect()
}

/// A built-in name, a user template name, or a path to a `.json` file.
pub fn resolve(name_or_path: &str) -> Result<Template> {
    if name_or_path.ends_with(".json") || name_or_path.contains('/') {
        return load_file(Path::new(name_or_path));
    }
    if let Some(t) = builtins().into_iter().find(|t| t.name == name_or_path) {
        return Ok(t);
    }
    let user = user_dir().join(format!("{name_or_path}.json"));
    if user.exists() {
        return load_file(&user);
    }
    Err(CmuxError::usage(format!(
        "unknown layout {name_or_path:?} — known: {}",
        known_names().join(", ")
    ))
    .with_suggestions(vec!["Run `cmux-axi layout list`".into()]))
}

// ---------------------------------------------------------------------------
// Validation + geometry
// ---------------------------------------------------------------------------

fn sums_to_one(v: &[f64]) -> bool {
    (v.iter().sum::<f64>() - 1.0).abs() <= 0.01 && v.iter().all(|x| *x > 0.0)
}

pub fn validate(t: &Template) -> Result<()> {
    let bad = |m: String| CmuxError::usage(format!("layout {:?}: {m}", t.name));
    if t.name.is_empty() {
        return Err(bad("name is required".into()));
    }
    match (&t.rows, &t.tree) {
        (Some(_), Some(_)) => return Err(bad("give rows or tree, not both".into())),
        (None, None) => return Err(bad("give rows or tree".into())),
        (Some(rows), None) => {
            if rows.is_empty() || rows.iter().any(|r| r.panes == 0) {
                return Err(bad("every row needs at least one pane".into()));
            }
            let heights: Vec<f64> = rows.iter().filter_map(|r| r.height).collect();
            if !heights.is_empty() {
                if heights.len() != rows.len() {
                    return Err(bad("give a height for every row or for none".into()));
                }
                if !sums_to_one(&heights) {
                    return Err(bad(format!("row heights must sum to 1 (got {:.2})", heights.iter().sum::<f64>())));
                }
            }
            for (i, r) in rows.iter().enumerate() {
                if let Some(w) = &r.widths {
                    if w.len() != r.panes {
                        return Err(bad(format!("row {i}: {} widths for {} panes", w.len(), r.panes)));
                    }
                    if !sums_to_one(w) {
                        return Err(bad(format!("row {i}: widths must sum to 1")));
                    }
                }
            }
        }
        (None, Some(tree)) => {
            let leaves = tree_leaf_order(tree)?;
            let n = leaves.len();
            let mut seen = vec![false; n];
            for s in &leaves {
                if *s >= n || seen[*s] {
                    return Err(bad(format!("tree slots must be exactly 0..{} once each", n.saturating_sub(1))));
                }
                seen[*s] = true;
            }
        }
    }
    let n = slots(t);
    let mut ds = t.dev_slots.clone();
    ds.sort_unstable();
    ds.dedup();
    if ds.len() != t.dev_slots.len() || ds.iter().any(|s| *s >= n) {
        return Err(bad(format!("dev_slots must be distinct slots below {n}")));
    }
    for (role, s) in &t.default_seats {
        if *s >= n {
            return Err(bad(format!("default_seats.{role} = {s} is not a slot (have {n})")));
        }
    }
    Ok(())
}

/// Number of slots (leaf panes).
pub fn slots(t: &Template) -> usize {
    match (&t.rows, &t.tree) {
        (Some(rows), _) => rows.iter().map(|r| r.panes).sum(),
        (None, Some(tree)) => tree_leaf_order(tree).map(|v| v.len()).unwrap_or(0),
        _ => 0,
    }
}

/// The slot at each leaf position of the compiled tree, in tree (= cmux pane index)
/// order. `rows` templates are the identity; `tree` templates follow the user's tree.
pub fn leaf_order(t: &Template) -> Vec<usize> {
    match (&t.rows, &t.tree) {
        (Some(_), _) => (0..slots(t)).collect(),
        (None, Some(tree)) => tree_leaf_order(tree).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn tree_leaf_order(v: &Value) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    walk(v, &mut out)?;
    return Ok(out);

    fn walk(v: &Value, out: &mut Vec<usize>) -> Result<()> {
        if let Some(s) = v.get("slot") {
            let n = s
                .as_u64()
                .ok_or_else(|| CmuxError::usage("tree: slot must be a non-negative integer"))?;
            out.push(n as usize);
            return Ok(());
        }
        match v.get("children").and_then(|c| c.as_array()) {
            Some(ch) => {
                for c in ch {
                    walk(c, out)?;
                }
                Ok(())
            }
            None => Err(CmuxError::usage(
                "tree: every node needs children or a slot",
            )),
        }
    }
}

/// Nested binary splits over `items`, first child's share = its weight / remaining.
fn nest(direction: &str, weights: &[f64], mut items: Vec<Value>) -> Value {
    if items.len() == 1 {
        return items.remove(0);
    }
    let total: f64 = weights.iter().sum();
    let first = items.remove(0);
    let rest = nest(direction, &weights[1..], items);
    let split = (weights[0] / total * 10000.0).round() / 10000.0;
    json!({ "direction": direction, "split": split, "children": [first, rest] })
}

/// Build the cmux `--layout` tree, placing `leaves[n]` (a `{"pane": …}` value) at slot n.
pub fn compile(t: &Template, leaves: Vec<Value>) -> Value {
    if let Some(rows) = &t.rows {
        let mut it = leaves.into_iter();
        let heights: Vec<f64> = rows
            .iter()
            .map(|r| r.height.unwrap_or(1.0 / rows.len() as f64))
            .collect();
        let row_values: Vec<Value> = rows
            .iter()
            .map(|r| {
                let widths = r
                    .widths
                    .clone()
                    .unwrap_or_else(|| vec![1.0 / r.panes as f64; r.panes]);
                let panes: Vec<Value> = (0..r.panes).map(|_| it.next().unwrap_or(bare())).collect();
                nest("horizontal", &widths, panes)
            })
            .collect();
        return nest("vertical", &heights, row_values);
    }
    let tree = t.tree.clone().unwrap_or(Value::Null);
    return fill(&tree, &leaves);

    fn fill(v: &Value, leaves: &[Value]) -> Value {
        if let Some(s) = v.get("slot").and_then(|s| s.as_u64()) {
            return leaves.get(s as usize).cloned().unwrap_or(bare());
        }
        let mut node = v.clone();
        if let Some(ch) = v.get("children").and_then(|c| c.as_array()) {
            node["children"] = Value::Array(ch.iter().map(|c| fill(c, leaves)).collect());
        }
        node
    }
}

/// A slot nobody sits in still needs a pane.
pub fn bare() -> Value {
    json!({ "pane": { "surfaces": [ { "type": "terminal" } ] } })
}

/// ASCII picture of a `rows` template; `labels[n]` is printed inside slot n.
pub fn diagram(t: &Template, labels: &[String]) -> String {
    let rows = match &t.rows {
        Some(r) => r,
        None => return "(tree layout — see JSON)".to_string(),
    };
    const W: usize = 60;
    let mut out = String::new();
    let mut slot = 0;
    for r in rows {
        let widths = r
            .widths
            .clone()
            .unwrap_or_else(|| vec![1.0 / r.panes as f64; r.panes]);
        let mut cols: Vec<usize> = widths.iter().map(|w| ((W as f64) * w).round() as usize).collect();
        let diff = W as i64 - cols.iter().sum::<usize>() as i64;
        if let Some(last) = cols.last_mut() {
            *last = (*last as i64 + diff).max(3) as usize;
        }
        let bar = |l: &str, m: &str, rgt: &str| {
            let mut s = String::from(l);
            for (i, c) in cols.iter().enumerate() {
                s.push_str(&"─".repeat(*c));
                s.push_str(if i + 1 == cols.len() { rgt } else { m });
            }
            s
        };
        out.push_str(&bar("┌", "┬", "┐"));
        out.push('\n');
        let mut line = String::from("│");
        for c in &cols {
            let label = labels.get(slot).cloned().unwrap_or_else(|| slot.to_string());
            let mut text: String = label.chars().take(c.saturating_sub(1)).collect();
            text.insert(0, ' ');
            let pad = c.saturating_sub(text.chars().count());
            line.push_str(&text);
            line.push_str(&" ".repeat(pad));
            line.push('│');
            slot += 1;
        }
        if let Some(h) = r.height {
            line.push_str(&format!("  {:.0}%", h * 100.0));
        }
        out.push_str(&line);
        out.push('\n');
        out.push_str(&bar("└", "┴", "┘"));
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(t: &[(usize, Option<f64>)]) -> Template {
        Template {
            name: "t".into(),
            summary: String::new(),
            rows: Some(t.iter().map(|(p, h)| Row { panes: *p, height: *h, widths: None }).collect()),
            tree: None,
            dev_slots: vec![],
            default_seats: BTreeMap::new(),
        }
    }

    fn leaves(n: usize) -> Vec<Value> {
        (0..n).map(|i| json!({ "pane": { "surfaces": [ { "type": "terminal", "command": format!("slot{i}") } ] } })).collect()
    }

    /// Commands of the leaf panes in tree order.
    fn leaf_commands(v: &Value, out: &mut Vec<String>) {
        if let Some(p) = v.get("pane") {
            out.push(p["surfaces"][0]["command"].as_str().unwrap_or("").to_string());
            return;
        }
        for c in v["children"].as_array().unwrap() {
            leaf_commands(c, out);
        }
    }

    #[test]
    fn builtin_2by2_is_valid_with_four_slots() {
        let t = resolve("2by2").unwrap();
        validate(&t).unwrap();
        assert_eq!(slots(&t), 4);
        assert_eq!(t.dev_slots, vec![2, 3]);
        assert_eq!(t.default_seats["coordinator"], 0);
        assert_eq!(t.default_seats["planner"], 0);
        assert_eq!(t.default_seats["brainstorm"], 1);
        assert_eq!(leaf_order(&t), vec![0, 1, 2, 3]);
    }

    #[test]
    fn default_is_3by2_and_builtins_validate() {
        assert_eq!(DEFAULT, "3by2");
        for t in builtins() {
            validate(&t).unwrap();
        }
        let t = resolve("3by2").unwrap();
        assert_eq!(slots(&t), 5);
        assert_eq!(t.dev_slots, vec![3, 4]);
        assert_eq!(t.rows.as_ref().unwrap()[0].height, Some(0.6));
    }

    #[test]
    fn rows_compile_to_equal_splits_in_slot_order() {
        let t = rows(&[(3, Some(0.6)), (2, Some(0.4))]);
        let tree = compile(&t, leaves(5));
        assert_eq!(tree["direction"], "vertical");
        assert_eq!(tree["split"], 0.6);
        let top = &tree["children"][0];
        assert_eq!(top["direction"], "horizontal");
        assert_eq!(top["split"], 0.3333);
        assert_eq!(top["children"][1]["split"], 0.5);
        assert_eq!(tree["children"][1]["split"], 0.5);
        let mut cmds = Vec::new();
        leaf_commands(&tree, &mut cmds);
        assert_eq!(cmds, vec!["slot0", "slot1", "slot2", "slot3", "slot4"]);
    }

    #[test]
    fn single_row_single_pane_is_just_the_pane() {
        let t = rows(&[(1, None)]);
        let tree = compile(&t, leaves(1));
        assert_eq!(tree["pane"]["surfaces"][0]["command"], "slot0");
    }

    #[test]
    fn tree_template_fills_slots_and_reports_leaf_order() {
        let t = Template {
            name: "col".into(),
            summary: String::new(),
            rows: None,
            tree: Some(json!({"direction":"horizontal","split":0.5,"children":[
                {"direction":"vertical","split":0.5,"children":[{"slot":0},{"slot":2}]},
                {"direction":"vertical","split":0.5,"children":[{"slot":1},{"slot":3}]}]})),
            dev_slots: vec![2, 3],
            default_seats: BTreeMap::new(),
        };
        validate(&t).unwrap();
        assert_eq!(slots(&t), 4);
        assert_eq!(leaf_order(&t), vec![0, 2, 1, 3]);
        let mut cmds = Vec::new();
        leaf_commands(&compile(&t, leaves(4)), &mut cmds);
        assert_eq!(cmds, vec!["slot0", "slot2", "slot1", "slot3"]);
    }

    #[test]
    fn validation_rejects_bad_templates() {
        let mut t = rows(&[(2, Some(0.7)), (2, Some(0.7))]);
        assert!(validate(&t).unwrap_err().message.contains("sum to 1"));
        t = rows(&[(2, None)]);
        t.dev_slots = vec![5];
        assert!(validate(&t).unwrap_err().message.contains("dev_slots"));
        t = rows(&[(2, None)]);
        t.default_seats.insert("planner".into(), 9);
        assert!(validate(&t).unwrap_err().message.contains("default_seats.planner"));
        t = rows(&[(2, None)]);
        t.tree = Some(json!({"slot":0}));
        assert!(validate(&t).unwrap_err().message.contains("not both"));
        let both_none = Template { rows: None, ..rows(&[(1, None)]) };
        assert!(validate(&both_none).is_err());
    }

    #[test]
    fn unknown_name_lists_known_layouts() {
        let e = resolve("nope").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.message.contains("2by2"), "{}", e.message);
    }

    #[test]
    fn json_round_trips_without_optional_fields() {
        let t = rows(&[(3, None), (2, None)]);
        let s = serde_json::to_string(&t).unwrap();
        assert!(!s.contains("height"), "{s}");
        assert_eq!(serde_json::from_str::<Template>(&s).unwrap(), t);
        assert!(serde_json::from_str::<Template>(r#"{"name":"x","rows":[{"panes":1}],"bogus":1}"#).is_err());
    }

    #[test]
    fn diagram_draws_one_box_per_row() {
        let t = rows(&[(3, Some(0.6)), (2, Some(0.4))]);
        let d = diagram(&t, &["0 coordinator".into(), "1 planner".into(), "2 brainstorm".into(), "3 devs".into(), "4 devs".into()]);
        assert_eq!(d.lines().count(), 6, "{d}");
        assert!(d.contains("│ 0 coordinator"));
        assert!(d.contains("60%") && d.contains("40%"));
    }
}
