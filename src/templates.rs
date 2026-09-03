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

// ---------------------------------------------------------------------------
// Authoring (`layout create` / `layout rm`)
// ---------------------------------------------------------------------------

/// `[a-z0-9][a-z0-9_-]*`, at most 32 chars.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    name.len() <= 32 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn parse_fracs(what: &str, s: &str) -> Result<Vec<f64>> {
    s.split(',')
        .map(|x| x.trim().parse::<f64>().map_err(|_| CmuxError::usage(format!("{what}: {x:?} is not a number"))))
        .collect()
}

/// `--rows 3,2` with optional `--heights 0.6,0.4` and `--widths 0.5,0.25,0.25/0.5,0.5`.
pub fn parse_rows(rows: &str, heights: Option<&str>, widths: Option<&str>) -> Result<Vec<Row>> {
    let counts: Vec<usize> = rows
        .split(',')
        .map(|x| x.trim().parse::<usize>().map_err(|_| CmuxError::usage(format!("--rows: {x:?} is not a pane count"))))
        .collect::<Result<_>>()?;
    let heights: Vec<Option<f64>> = match heights {
        Some(h) => {
            let v = parse_fracs("--heights", h)?;
            if v.len() != counts.len() {
                return Err(CmuxError::usage(format!("--heights: {} values for {} rows", v.len(), counts.len())));
            }
            v.into_iter().map(Some).collect()
        }
        None => vec![None; counts.len()],
    };
    let widths: Vec<Option<Vec<f64>>> = match widths {
        Some(w) => {
            let v: Vec<Vec<f64>> = w.split('/').map(|row| parse_fracs("--widths", row)).collect::<Result<_>>()?;
            if v.len() != counts.len() {
                return Err(CmuxError::usage(format!("--widths: {} rows for {} rows (separate rows with /)", v.len(), counts.len())));
            }
            v.into_iter().map(Some).collect()
        }
        None => vec![None; counts.len()],
    };
    Ok(counts
        .into_iter()
        .zip(heights)
        .zip(widths)
        .map(|((panes, height), widths)| Row { panes, height, widths })
        .collect())
}

/// `3,4` → slots.
pub fn parse_slots(what: &str, s: &str) -> Result<Vec<usize>> {
    s.split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| x.trim().parse::<usize>().map_err(|_| CmuxError::usage(format!("{what}: {x:?} is not a slot"))))
        .collect()
}

/// `coordinator=0,planner=1` → default seats.
pub fn parse_seats(s: &str) -> Result<BTreeMap<String, usize>> {
    let mut out = BTreeMap::new();
    for part in s.split(',').filter(|x| !x.trim().is_empty()) {
        let (role, slot) = part
            .split_once('=')
            .ok_or_else(|| CmuxError::usage(format!("--seat: {part:?} is not role=slot")))?;
        let n = slot.trim().parse::<usize>().map_err(|_| CmuxError::usage(format!("--seat: {slot:?} is not a slot")))?;
        out.insert(role.trim().to_string(), n);
    }
    Ok(out)
}

/// A pane's pixel frame, as `cmux list-panes --json` reports it for a focused workspace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Recover a row grid from live pane frames: panes grouped by `y` (±4 px) are a row,
/// every pane in a row shares its height, rows tile the container height and each row
/// tiles its width. Anything else is refused — write it as a `tree` template.
pub fn rows_from_frames(container_w: f64, container_h: f64, frames: &[Frame]) -> Result<Vec<Row>> {
    const TOL: f64 = 4.0;
    if container_w <= 0.0 || container_h <= 0.0 || frames.is_empty() {
        return Err(CmuxError::usage(
            "pane frames are zero — the workspace must be visible: `cmux workspace select --workspace <ref>` first",
        ));
    }
    let mut sorted: Vec<Frame> = frames.to_vec();
    sorted.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap().then(a.x.partial_cmp(&b.x).unwrap()));
    let mut rows_f: Vec<Vec<Frame>> = Vec::new();
    for f in sorted {
        match rows_f.last_mut() {
            Some(row) if (row[0].y - f.y).abs() <= TOL => row.push(f),
            _ => rows_f.push(vec![f]),
        }
    }
    let not_grid = |why: String| {
        CmuxError::usage(format!("not a row grid ({why}) — write it as a `tree` template"))
    };
    let mut rows = Vec::new();
    let mut total_h = 0.0;
    for (i, row) in rows_f.iter_mut().enumerate() {
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let h = row[0].h;
        if row.iter().any(|f| (f.h - h).abs() > TOL) {
            return Err(not_grid(format!("row {i}: panes differ in height")));
        }
        let w_sum: f64 = row.iter().map(|f| f.w).sum();
        if (w_sum - container_w).abs() > TOL * row.len() as f64 {
            return Err(not_grid(format!("row {i}: panes span {w_sum:.0} of {container_w:.0} px")));
        }
        total_h += h;
        let widths: Vec<f64> = row.iter().map(|f| (f.w / w_sum * 1000.0).round() / 1000.0).collect();
        let equal = widths.iter().all(|w| (w - widths[0]).abs() <= 0.01);
        rows.push(Row { panes: row.len(), height: Some((h / container_h * 1000.0).round() / 1000.0), widths: if equal { None } else { Some(widths) } });
    }
    if (total_h - container_h).abs() > TOL * rows.len() as f64 {
        return Err(not_grid(format!("rows span {total_h:.0} of {container_h:.0} px")));
    }
    let hs: Vec<f64> = rows.iter().filter_map(|r| r.height).collect();
    if hs.iter().all(|h| (h - hs[0]).abs() <= 0.01) {
        for r in rows.iter_mut() {
            r.height = None;
        }
    }
    Ok(rows)
}

/// Write a user template as `<dir>/<name>.json`. Built-in names are reserved; an
/// existing file needs `force`.
pub fn write_user(dir: &Path, t: &Template, force: bool) -> Result<PathBuf> {
    if !valid_name(&t.name) {
        return Err(CmuxError::usage(format!("layout name {:?}: use [a-z0-9][a-z0-9_-]*, at most 32 chars", t.name)));
    }
    if builtins().iter().any(|b| b.name == t.name) {
        return Err(CmuxError::usage(format!("layout {:?} is built-in (reserved); pick another name", t.name)));
    }
    validate(t)?;
    let path = dir.join(format!("{}.json", t.name));
    if path.exists() && !force {
        return Err(CmuxError::usage(format!("{} exists — pass --force to replace it", path.display())));
    }
    std::fs::create_dir_all(dir).map_err(|e| CmuxError::operational(format!("cannot create {}: {e}", dir.display()), "LAYOUT_WRITE"))?;
    let body = serde_json::to_string_pretty(t).unwrap_or_default() + "\n";
    std::fs::write(&path, body).map_err(|e| CmuxError::operational(format!("cannot write {}: {e}", path.display()), "LAYOUT_WRITE"))?;
    Ok(path)
}

/// Remove `<dir>/<name>.json`. `Ok(None)` when there was nothing to remove.
pub fn remove_user(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    if builtins().iter().any(|b| b.name == name) {
        return Err(CmuxError::usage(format!("layout {name:?} is built-in and cannot be removed")));
    }
    let path = dir.join(format!("{name}.json"));
    if !path.exists() {
        return Ok(None);
    }
    std::fs::remove_file(&path).map_err(|e| CmuxError::operational(format!("cannot remove {}: {e}", path.display()), "LAYOUT_WRITE"))?;
    Ok(Some(path))
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
    fn rows_flag_parses_and_compiles() {
        let r = parse_rows("3,2", Some("0.6,0.4"), Some("0.5,0.25,0.25/0.5,0.5")).unwrap();
        assert_eq!(r[0], Row { panes: 3, height: Some(0.6), widths: Some(vec![0.5, 0.25, 0.25]) });
        assert_eq!(r[1].panes, 2);
        let t = Template { name: "w".into(), summary: String::new(), rows: Some(r), tree: None, dev_slots: vec![3, 4], default_seats: BTreeMap::new() };
        validate(&t).unwrap();
        let tree = compile(&t, leaves(5));
        assert_eq!(tree["children"][0]["split"], 0.5); // first pane's width share
        assert!(parse_rows("3,x", None, None).is_err());
        assert!(parse_rows("3,2", Some("0.6"), None).unwrap_err().message.contains("1 values for 2 rows"));
        assert_eq!(parse_slots("--dev-slots", "3,4").unwrap(), vec![3, 4]);
        assert_eq!(parse_seats("coordinator=0, planner=1").unwrap()["planner"], 1);
        assert!(parse_seats("coordinator").is_err());
    }

    #[test]
    fn heights_must_sum_to_one() {
        let r = parse_rows("2,2", Some("0.7,0.7"), None).unwrap();
        let t = Template { name: "bad".into(), summary: String::new(), rows: Some(r), tree: None, dev_slots: vec![], default_seats: BTreeMap::new() };
        assert!(validate(&t).unwrap_err().message.contains("sum to 1"));
    }

    /// The frames `cmux list-panes --json` reported for a 3-over-2 workspace on 2026-09-03
    /// (container 1488×988; x/y are window-relative, not container-relative).
    fn probe_frames() -> Vec<Frame> {
        vec![
            Frame { x: 240.0, y: 28.0, w: 496.0, h: 593.0 },
            Frame { x: 736.0, y: 28.0, w: 496.0, h: 593.0 },
            Frame { x: 1232.0, y: 28.0, w: 496.0, h: 593.0 },
            Frame { x: 240.0, y: 621.0, w: 744.0, h: 395.0 },
            Frame { x: 984.0, y: 621.0, w: 744.0, h: 395.0 },
        ]
    }

    #[test]
    fn from_workspace_recovers_3by2() {
        let rows = rows_from_frames(1488.0, 988.0, &probe_frames()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].panes, rows[0].height, rows[0].widths.clone()), (3, Some(0.6), None));
        assert_eq!((rows[1].panes, rows[1].height), (2, Some(0.4)));
        // equal rows drop the heights entirely
        let eq = rows_from_frames(1000.0, 1000.0, &[Frame { x: 0.0, y: 0.0, w: 1000.0, h: 500.0 }, Frame { x: 0.0, y: 500.0, w: 1000.0, h: 500.0 }]).unwrap();
        assert!(eq.iter().all(|r| r.height.is_none()));
        // unequal widths are kept
        let uw = rows_from_frames(1000.0, 500.0, &[Frame { x: 0.0, y: 0.0, w: 750.0, h: 500.0 }, Frame { x: 750.0, y: 0.0, w: 250.0, h: 500.0 }]).unwrap();
        assert_eq!(uw[0].widths, Some(vec![0.75, 0.25]));
    }

    #[test]
    fn from_workspace_refuses_non_grid_and_zero_frames() {
        // L-shape: a tall left pane beside two stacked right panes.
        let l = vec![
            Frame { x: 0.0, y: 0.0, w: 500.0, h: 1000.0 },
            Frame { x: 500.0, y: 0.0, w: 500.0, h: 500.0 },
            Frame { x: 500.0, y: 500.0, w: 500.0, h: 500.0 },
        ];
        let e = rows_from_frames(1000.0, 1000.0, &l).unwrap_err();
        assert!(e.message.contains("not a row grid"), "{}", e.message);
        let e = rows_from_frames(0.0, 0.0, &probe_frames()).unwrap_err();
        assert!(e.message.contains("workspace select"), "{}", e.message);
    }

    #[test]
    fn user_templates_write_resolve_and_remove_but_never_builtins() {
        let dir = std::env::temp_dir().join(format!("cmux-axi-layouts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut t = rows(&[(4, Some(0.7)), (1, Some(0.3))]);
        t.name = "wide".into();
        t.dev_slots = vec![4];
        let path = write_user(&dir, &t, false).unwrap();
        assert_eq!(load_file(&path).unwrap(), t);
        assert!(write_user(&dir, &t, false).unwrap_err().message.contains("--force"));
        write_user(&dir, &t, true).unwrap();
        t.name = "3by2".into();
        assert!(write_user(&dir, &t, true).unwrap_err().message.contains("reserved"));
        t.name = "Bad Name".into();
        assert!(write_user(&dir, &t, true).unwrap_err().message.contains("layout name"));
        assert!(valid_name("my-layout_2") && !valid_name("-x") && !valid_name(""));
        assert!(remove_user(&dir, "2by2").unwrap_err().message.contains("built-in"));
        assert!(remove_user(&dir, "wide").unwrap().is_some());
        assert!(remove_user(&dir, "wide").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
