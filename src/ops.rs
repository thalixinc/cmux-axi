//! Command orchestration: provision, status, send, read, dev add/rm, teardown.
//! Every op prints TOON (default) or `--json` and returns `Result<()>`.

use crate::cmux;
use crate::crew;
use crate::error::{CmuxError, Result};
use crate::fleet::{self, FleetEntry};
use crate::layout::{self, Seating};
use crate::templates;
use crate::toon;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

const DESCRIPTION: &str = "Provision the codefactory crew layout in cmux";
const BIN: &str = "cmux-axi";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now() -> String {
    Command::new("date")
        .args(["+%Y-%m-%d"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn ws_title(project: &str) -> String {
    format!("cf-{project}")
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|c| c.join(path))
            .map_err(|e| CmuxError::operational(format!("cannot resolve cwd: {e}"), "CWD"))
    }
}

/// Resolve the state root: explicit `--state-dir`, else `<cwd>/.omp/state`.
/// The tab title for a role: `Coordinator`, `Planner`, `Brainstorm`, `Developer 3`
/// (plus ` · <specialty>` for a disposable developer).
pub fn title_for(role: &str, specialty: Option<&str>) -> String {
    let base = match role {
        "coordinator" => "Coordinator".to_string(),
        "planner" => "Planner".to_string(),
        "brainstorm" => "Brainstorm".to_string(),
        r if r.starts_with("dev-") => format!("Developer {}", &r[4..]),
        other => other.to_string(),
    };
    match specialty {
        Some(s) if !s.is_empty() => format!("{base} · {s}"),
        _ => base,
    }
}

#[cfg(test)]
mod title_tests {
    use super::title_for;
    #[test]
    fn titles_follow_the_role() {
        assert_eq!(title_for("coordinator", None), "Coordinator");
        assert_eq!(title_for("dev-3", None), "Developer 3");
        assert_eq!(title_for("dev-4", Some("rust")), "Developer 4 · rust");
        assert_eq!(title_for("dev-5", Some("")), "Developer 5");
        assert_eq!(title_for("qa", None), "qa");
    }
}

/// Rename a surface's tab; a failure is reported, never fatal (the crew still runs).
fn title_surface(workspace: &str, surface: &str, title: &str) {
    if let Err(e) = cmux::run(&["rename-tab", "--workspace", workspace, "--surface", surface, title]) {
        eprintln!("cmux-axi: could not title {surface} as {title:?}: {e}");
    }
}

/// `<home>/.omp/state` → `<home>`; anything else → the launch cwd.
fn cof_home_of(state: &Path, cwd: &Path) -> PathBuf {
    match (state.file_name(), state.parent().and_then(|p| p.file_name()), state.parent().and_then(|p| p.parent())) {
        (Some(s), Some(o), Some(home)) if s == "state" && o == ".omp" => home.to_path_buf(),
        _ => cwd.to_path_buf(),
    }
}

fn state_root(cwd: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(p) => absolute(p),
        None => Ok(absolute(cwd)?.join(".omp").join("state")),
    }
}

// ---------------------------------------------------------------------------
// provision
// ---------------------------------------------------------------------------

/// `req` is the crew spec with flags already merged in (see `main`): layout, harness,
/// cwd, and either `devs` (default crew) or explicit `seats`.
pub fn provision(project: &str, req: &crew::Request, state_dir: Option<&Path>, json: bool) -> Result<()> {
    cmux::ensure_installed()?;
    let template = templates::resolve(req.layout.as_deref().unwrap_or(templates::DEFAULT))?;
    let harness = req.harness.as_deref().unwrap_or("omp");
    let cwd_abs = absolute(Path::new(req.cwd.as_deref().unwrap_or(".")))?;
    let state = state_root(&cwd_abs, state_dir)?;
    std::fs::create_dir_all(&state).map_err(|e| {
        CmuxError::operational(format!("cannot create {}: {e}", state.display()), "STATE")
    })?;
    let state_str = state.to_string_lossy().to_string();
    let seating = if req.seats.is_empty() {
        layout::default_seating(&template, req.devs.unwrap_or(2))?
    } else {
        layout::seat(&template, &req.seats, req.dev_slots.as_deref())?
    };
    let dev_slots = req.dev_slots.clone().unwrap_or_else(|| template.dev_slots.clone());

    let title = ws_title(project);
    if let Ok(existing) = cmux::find_workspace_by_title(&title) {
        // Idempotent: already provisioned. A differing spec is reported, never applied.
        let rows = fleet::load(&state.join("fleet.md"))?;
        print_fleet(rows.iter().filter(|e| e.project == project).collect(), json);
        println!(
            "{}",
            toon::kv("already", &format!("{} ({})", title, existing.r#ref))
        );
        if let Some(rec) = crew::load(&state, project)? {
            if rec.layout != template.name || rec.seats != seating {
                println!("{}", toon::kv("drift", "spec differs from the provisioned crew (teardown to apply)"));
            }
        }
        return Ok(());
    }

    // The Chief-of-Staff home is the state root's grandparent (<home>/.omp/state).
    let cof_home = cof_home_of(&state, &cwd_abs);
    let spec = layout::CrewSpec {
        state_root: &state_str,
        harness,
        project,
        cof_home: &cof_home.to_string_lossy(),
    };
    let tree = layout::build(&spec, &template, &seating);
    let layout_json = serde_json::to_string(&tree)
        .map_err(|e| CmuxError::operational(format!("layout build failed: {e}"), "LAYOUT"))?;

    cmux::run(&[
        "new-workspace",
        "--name",
        &title,
        "--cwd",
        &cwd_abs.to_string_lossy(),
        "--layout",
        &layout_json,
    ])?;

    let ws = cmux::find_workspace_by_title(&title)?;
    // Note: workspace grouping (answer 7) is deferred — `workspace-group create
    // --from` also spawns a duplicate anchor workspace, so a flat `cf-<project>`
    // workspace is the clean v1 shape.

    let leaf_order = templates::leaf_order(&template);
    let entries = map_roles_to_surfaces(&ws.r#ref, project, &state_str, harness, &seating, &leaf_order)?;
    // Tab titles carry the agent title (the workspace already carries the project).
    for e in &entries {
        let title = seating
            .iter()
            .find(|s| s.role == e.role)
            .and_then(|s| s.title.clone())
            .unwrap_or_else(|| title_for(&e.role, None));
        title_surface(&ws.r#ref, &e.surface, &title);
    }
    let fleet_path = state.join("fleet.md");
    let mut all = fleet::load(&fleet_path)?;
    for e in &entries {
        fleet::upsert(&mut all, e.clone());
    }
    fleet::write(&fleet_path, &all)?;
    crew::write(
        &state,
        project,
        &crew::Record {
            layout: template.name.clone(),
            slots: templates::slots(&template),
            dev_slots,
            leaf_order,
            tree,
            seats: seating,
            harness: harness.to_string(),
            cwd: cwd_abs.to_string_lossy().to_string(),
        },
    )?;

    print_fleet(entries.iter().collect(), json);
    if !json {
        println!("{}", toon::kv("layout", &template.name));
        let help = vec![
            format!("Run `cmux-axi send {project} planner \"…\"` to steer"),
            "Run `cmux-axi status` for drift".to_string(),
        ];
        println!("\n{}", toon::help(&help));
    }
    Ok(())
}

/// Map the freshly-provisioned panes to seats. Pane index = layout tree leaf order
/// (`leaf_order[i]` is the slot at pane i); surfaces within a pane are the seats of that
/// slot in seat order. Pixel frames are all zero for an unfocused workspace, so geometry
/// is never consulted.
fn map_roles_to_surfaces(
    workspace: &str,
    project: &str,
    state_str: &str,
    harness: &str,
    seating: &Seating,
    leaf_order: &[usize],
) -> Result<Vec<FleetEntry>> {
    let panes = panes_by_index(workspace)?;
    if panes.len() != leaf_order.len() {
        return Err(CmuxError::operational(
            format!("expected {} panes in {workspace}, found {}", leaf_order.len(), panes.len()),
            "LAYOUT_UNEXPECTED",
        ));
    }

    let started = now();
    let mut entries: Vec<FleetEntry> = Vec::new();
    for (i, pane) in panes.iter().enumerate() {
        let slot = leaf_order[i];
        let seats: Vec<&layout::Seat> = seating.iter().filter(|s| s.slot == slot).collect();
        if seats.is_empty() {
            continue; // a bare-terminal slot
        }
        let mut surfaces = cmux::list_pane_surfaces(&pane.r#ref, workspace)?.surfaces;
        surfaces.sort_by_key(|s| s.index.unwrap_or(0));
        if surfaces.len() < seats.len() {
            return Err(CmuxError::operational(
                format!("slot {slot} ({}) has {} surfaces for {} seats", pane.r#ref, surfaces.len(), seats.len()),
                "LAYOUT_UNEXPECTED",
            ));
        }
        for (seat, surface) in seats.iter().zip(surfaces.iter()) {
            let session = if seat.resumable {
                session_id(state_str, &seat.role, harness)
            } else {
                "ephemeral".to_string()
            };
            entries.push(FleetEntry {
                role: seat.role.clone(),
                project: project.to_string(),
                surface: surface.r#ref.clone(),
                session,
                status: "active".to_string(),
                started: started.clone(),
            });
        }
    }
    Ok(entries)
}

/// A workspace's panes in cmux index order (= layout tree leaf order).
fn panes_by_index(workspace: &str) -> Result<Vec<cmux::Pane>> {
    let mut panes = cmux::list_panes(workspace)?.panes;
    panes.sort_by_key(|p| p.index.unwrap_or(0));
    Ok(panes)
}

/// The `session` field for a role: a session dir for resumable roles, or
/// `ephemeral`. Only `omp` gets real session isolation today.
fn session_id(state_str: &str, role: &str, harness: &str) -> String {
    match harness {
        "omp" => format!("{}/sessions/{role}", state_str),
        _ => "ephemeral".to_string(),
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub fn status(project: Option<&str>, state_dir: Option<&Path>, json: bool) -> Result<()> {
    cmux::ensure_installed()?;
    let cwd = std::env::current_dir().map_err(|e| CmuxError::operational(e.to_string(), "CWD"))?;
    let state = state_root(&cwd, state_dir)?;
    let mut rows = fleet::load(&state.join("fleet.md"))?;

    // Liveness: a surface that cmux can no longer read is dead; one that came back is active.
    // fleet.md is rewritten only when something changed.
    let mut changed = false;
    for e in rows.iter_mut() {
        let alive = cmux::run(&["read-screen", "--surface", &e.surface]).is_ok();
        let next = match (alive, e.status.as_str()) {
            (false, _) => "dead",
            (true, "dead") | (true, "unknown") => "active",
            (true, s) => s,
        };
        if next != e.status {
            e.status = next.to_string();
            changed = true;
        }
    }
    if changed {
        fleet::write(&state.join("fleet.md"), &rows)?;
    }

    let filtered: Vec<&FleetEntry> = match project {
        Some(p) => rows.iter().filter(|e| e.project == p).collect(),
        None => rows.iter().collect(),
    };

    let live = cmux::list_workspaces()?;
    let live_refs: std::collections::HashSet<String> =
        live.workspaces.iter().map(|w| w.r#ref.clone()).collect();
    // Surface liveness: a workspace title present means the crew is provisioned.
    let title = project.map(ws_title);
    let provisioned = title
        .as_deref()
        .map(|t| {
            live.workspaces
                .iter()
                .any(|w| w.custom_title.as_deref() == Some(t))
        })
        .unwrap_or(false);

    let record = match project {
        Some(p) => crew::load(&state, p)?,
        None => None,
    };
    let layout_name = record.as_ref().map(|r| r.layout.clone());

    if json {
        let arr: Vec<_> = filtered
            .iter()
            .map(|e| {
                json!({
                    "role": e.role, "project": e.project, "surface": e.surface,
                    "session": e.session, "status": e.status, "started": e.started,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 1, "provisioned": provisioned, "fleet": arr,
                "layout": layout_name, "crew": record,
                "live_workspace_refs": live_refs.iter().collect::<Vec<_>>(),
            }))
            .unwrap()
        );
        return Ok(());
    }

    let mut blocks = vec![toon::header(BIN, DESCRIPTION)];
    let rows: Vec<Vec<String>> = filtered
        .iter()
        .map(|e| {
            vec![
                e.role.clone(),
                e.project.clone(),
                e.surface.clone(),
                e.session.clone(),
                e.status.clone(),
            ]
        })
        .collect();
    blocks.push(toon::list(
        "fleet",
        &["role", "project", "surface", "session", "status"],
        &rows,
    ));
    if let Some(l) = &layout_name {
        blocks.push(toon::kv("layout", l));
    }
    let help = if provisioned {
        vec!["Run `cmux-axi teardown <project>` to remove the crew".to_string()]
    } else if let Some(p) = project {
        vec![format!("Not provisioned — run `cmux-axi provision {p}`")]
    } else {
        vec!["Run `cmux-axi provision <project>` to provision a crew".to_string()]
    };
    blocks.push(toon::help(&help));
    println!("{}", toon::join(&blocks));
    Ok(())
}

// ---------------------------------------------------------------------------
// send / read
// ---------------------------------------------------------------------------

fn resolve_surface(project: &str, role: &str, state_dir: Option<&Path>) -> Result<String> {
    let cwd = std::env::current_dir().map_err(|e| CmuxError::operational(e.to_string(), "CWD"))?;
    let state = state_root(&cwd, state_dir)?;
    let rows = fleet::load(&state.join("fleet.md"))?;
    // `cof` is the Chief of Staff: one row for the whole home (recorded by its session start),
    // reachable from any project's crew.
    let hit = fleet::find(&rows, project, role)
        .or_else(|| (role == "cof").then(|| rows.iter().find(|e| e.role == "cof")).flatten());
    hit.map(|e| e.surface.clone())
        .ok_or_else(|| {
            CmuxError::operational(
                format!("no fleet entry for {project}/{role}"),
                "ROLE_NOT_FOUND",
            )
            .with_suggestions(vec![
                format!("Run `cmux-axi status --project {project}`"),
                format!("Or `cmux-axi provision {project}` if not yet provisioned"),
            ])
        })
}
fn resolve_workspace(project: &str) -> Result<String> {
    cmux::find_workspace_by_title(&ws_title(project)).map(|w| w.r#ref)
}

pub fn send(
    project: &str,
    role: &str,
    text: &str,
    state_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    let surface = resolve_surface(project, role, state_dir)?;
    // Surface refs are global; the crew workspace is only a hint. The Chief of Staff's
    // surface lives outside the crew workspace, so address it by surface alone.
    let ws = if role == "cof" { None } else { resolve_workspace(project).ok() };
    let mut send_args = vec!["send"];
    let mut key_args = vec!["send-key"];
    if let Some(w) = &ws {
        send_args.extend(["--workspace", w]);
        key_args.extend(["--workspace", w]);
    }
    send_args.extend(["--surface", &surface, text]);
    key_args.extend(["--surface", &surface, "enter"]);
    cmux::run(&send_args)?;
    cmux::run(&key_args)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({"ok": true, "action": "send", "surface": surface}))
                .unwrap()
        );
    } else {
        println!("ok: send {project}/{role} -> {surface}");
    }
    Ok(())
}

pub fn read(project: &str, role: &str, state_dir: Option<&Path>, json: bool) -> Result<()> {
    let surface = resolve_surface(project, role, state_dir)?;
    let ws = resolve_workspace(project)?;
    let text = cmux::run(&["read-screen", "--workspace", &ws, "--surface", &surface])?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({"surface": surface, "screen": text})).unwrap()
        );
    } else {
        println!("{}", text);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// dev add / rm
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)] // flat CLI dispatch surface
pub fn dev_add(
    project: &str,
    cwd: &Path,
    harness: &str,
    specialty: Option<&str>,
    id: Option<&str>,
    seed_prompt: Option<&Path>,
    worktree: bool,
    state_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    cmux::ensure_installed()?;
    let cwd_abs = absolute(cwd)?;
    let state = state_root(&cwd_abs, state_dir)?;
    let title = ws_title(project);
    let ws = cmux::find_workspace_by_title(&title)?;

    // Pick the developer pane with the fewest surfaces (balance). The crew record says
    // which slots take developers; a crew without one (cmux-axi ≤ 0.2.4) falls back to
    // "the last two panes", which is what that version assumed.
    let record = crew::load(&state, project)?;
    let mut panes = panes_by_index(&ws.r#ref)?;
    let candidates: Vec<cmux::Pane> = match &record {
        Some(rec) => panes
            .into_iter()
            .enumerate()
            .filter(|(i, _)| rec.leaf_order.get(*i).map(|s| rec.dev_slots.contains(s)).unwrap_or(false))
            .map(|(_, p)| p)
            .collect(),
        None => {
            if panes.len() < 4 {
                return Err(CmuxError::operational(
                    "crew layout not fully provisioned",
                    "LAYOUT_UNEXPECTED",
                ));
            }
            panes.split_off(panes.len() - 2)
        }
    };
    let pane = candidates
        .iter()
        .min_by_key(|p| p.surface_refs.len())
        .ok_or_else(|| CmuxError::operational("layout has no developer slots", "LAYOUT_UNEXPECTED"))?;

    // Worktree (git-native) if requested and a git repo is present.
    let mut work_dir = cwd_abs.clone();
    let mut worktree_path: Option<PathBuf> = None;
    if worktree {
        let id_for_path = id.unwrap_or("dev");
        let wt = cwd_abs.join(".omp").join("worktrees").join(id_for_path);
        Command::new("git")
            .args([
                "worktree",
                "add",
                &wt.to_string_lossy(),
                "-b",
                &format!("cmux-axi/{id_for_path}"),
            ])
            .current_dir(&cwd_abs)
            .output()
            .map_err(|e| {
                CmuxError::operational(format!("git worktree add failed: {e}"), "WORKTREE")
            })?;
        work_dir = wt.clone();
        worktree_path = Some(wt);
    }

    // Create the surface (bare terminal at the work dir), then launch harness.
    cmux::run(&[
        "new-surface",
        "--type",
        "terminal",
        "--workspace",
        &ws.r#ref,
        "--pane",
        &pane.r#ref,
        "--working-directory",
        &work_dir.to_string_lossy(),
    ])?;

    // Resolve the new surface: it is the (now one-more) surface in the pane.
    let surfaces = cmux::list_pane_surfaces(&pane.r#ref, &ws.r#ref)?.surfaces;
    let new_surface = surfaces
        .iter()
        .max_by_key(|s| s.index.unwrap_or(0))
        .ok_or_else(|| CmuxError::operational("no surface created", "SURFACE"))?;

    // Mint a unique dev id if none given (the pane is told its role at launch).
    let mut all = fleet::load(&state.join("fleet.md"))?;
    let dev_id = match id {
        Some(i) => i.to_string(),
        None => {
            let max = all
                .iter()
                .filter(|e| e.project == project && e.role.starts_with("dev-"))
                .filter_map(|e| e.role.trim_start_matches("dev-").parse::<usize>().ok())
                .max()
                .unwrap_or(0);
            format!("dev-{}", max + 1)
        }
    };
    let cof_home = cof_home_of(&state, &cwd_abs);
    let state_s = state.to_string_lossy().to_string();
    let spec = layout::CrewSpec {
        state_root: &state_s,
        harness,
        project,
        cof_home: &cof_home.to_string_lossy(),
    };
    let mut cmd = layout::harness_command(&spec, &dev_id, false);
    if let Some(seed) = seed_prompt {
        let seed_abs = absolute(seed)?;
        cmd = format!("{cmd} @{}", seed_abs.to_string_lossy());
    }
    cmux::run(&[
        "send",
        "--workspace",
        &ws.r#ref,
        "--surface",
        &new_surface.r#ref,
        &cmd,
    ])?;
    cmux::run(&[
        "send-key",
        "--workspace",
        &ws.r#ref,
        "--surface",
        &new_surface.r#ref,
        "enter",
    ])?;
    let entry = FleetEntry {
        role: dev_id.clone(),
        project: project.to_string(),
        surface: new_surface.r#ref.clone(),
        session: "ephemeral".to_string(),
        status: "active".to_string(),
        started: now(),
    };
    fleet::upsert(&mut all, entry.clone());
    fleet::write(&state.join("fleet.md"), &all)?;
    title_surface(&ws.r#ref, &new_surface.r#ref, &title_for(&dev_id, specialty));

    let note = if let Some(p) = worktree_path {
        format!("worktree={}", p.display())
    } else {
        "no-worktree".to_string()
    };
    let sp = specialty.unwrap_or("general").to_string();
    if json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "ok": true, "action": "dev-add", "dev": dev_id, "surface": new_surface.r#ref,
                "specialty": sp, "session": "ephemeral", "worktree": note,
            }))
            .unwrap()
        );
    } else {
        println!(
            "ok: dev add {project}/{dev_id} -> {} ({sp}, {note})",
            new_surface.r#ref
        );
    }
    Ok(())
}

pub fn dev_rm(
    project: &str,
    dev_id: &str,
    force: bool,
    state_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cwd = std::env::current_dir().map_err(|e| CmuxError::operational(e.to_string(), "CWD"))?;
    let state = state_root(&cwd, state_dir)?;
    let mut all = fleet::load(&state.join("fleet.md"))?;
    let entry = fleet::find(&all, project, dev_id).cloned().ok_or_else(|| {
        CmuxError::operational(
            format!("no fleet entry for {project}/{dev_id}"),
            "ROLE_NOT_FOUND",
        )
    })?;

    // Landed-work gate is a no-op unless a worktree is recorded; for v1 the
    // worktree path is not persisted in fleet, so the caller owns that check.
    // Closing the surface removes the window; force is accepted but surface
    // closure is the same either way today.
    let _ = force;
    let ws = resolve_workspace(project)?;
    cmux::run(&[
        "close-surface",
        "--workspace",
        &ws,
        "--surface",
        &entry.surface,
    ])?;
    fleet::remove(&mut all, project, dev_id);
    fleet::write(&state.join("fleet.md"), &all)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(
                &json!({"ok": true, "action": "dev-rm", "dev": dev_id, "surface": entry.surface})
            )
            .unwrap()
        );
    } else {
        println!("ok: dev rm {project}/{dev_id} (closed {})", entry.surface);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// teardown
// ---------------------------------------------------------------------------

pub fn teardown(project: &str, force: bool, state_dir: Option<&Path>, json: bool) -> Result<()> {
    cmux::ensure_installed()?;
    let title = ws_title(project);
    let ws = match cmux::find_workspace_by_title(&title) {
        Ok(w) => w,
        Err(_) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&json!({"ok": true, "already": true})).unwrap()
                );
            } else {
                println!("already: true (no workspace {title})");
            }
            return Ok(());
        }
    };
    let _ = force;
    cmux::run(&["close-workspace", "--workspace", &ws.r#ref])?;

    let cwd = std::env::current_dir().map_err(|e| CmuxError::operational(e.to_string(), "CWD"))?;
    let state = state_root(&cwd, state_dir)?;
    let mut all = fleet::load(&state.join("fleet.md"))?;
    all.retain(|e| e.project != project);
    fleet::write(&state.join("fleet.md"), &all)?;
    crew::remove(&state, project);

    if json {
        println!("{}", serde_json::to_string(&json!({"ok": true, "action": "teardown", "project": project, "workspace": ws.r#ref})).unwrap());
    } else {
        println!("ok: teardown {project} (closed {})", ws.r#ref);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// layout list / show
// ---------------------------------------------------------------------------

/// Slot labels for a diagram: `0 coordinator`, `3 devs`, or just the number.
fn slot_labels(t: &templates::Template) -> Vec<String> {
    (0..templates::slots(t))
        .map(|s| {
            let mut who: Vec<&str> = t
                .default_seats
                .iter()
                .filter(|(_, v)| **v == s)
                .map(|(k, _)| k.as_str())
                .collect();
            if t.dev_slots.contains(&s) {
                who.push("devs");
            }
            if who.is_empty() { s.to_string() } else { format!("{s} {}", who.join("+")) }
        })
        .collect()
}

pub fn layout_list(json: bool) -> Result<()> {
    let all = templates::list();
    if json {
        let arr: Vec<_> = all
            .iter()
            .map(|(t, src)| {
                json!({
                    "name": t.name, "source": src.to_string(), "slots": templates::slots(t),
                    "dev_slots": t.dev_slots, "default": t.name == templates::DEFAULT,
                    "summary": t.summary, "template": t,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({"layouts": arr, "user_dir": templates::user_dir()})).unwrap());
        return Ok(());
    }
    let rows: Vec<Vec<String>> = all
        .iter()
        .map(|(t, src)| {
            vec![
                t.name.clone(),
                src.to_string(),
                templates::slots(t).to_string(),
                t.dev_slots.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("+"),
                if t.name == templates::DEFAULT { "yes".into() } else { String::new() },
                t.summary.clone(),
            ]
        })
        .collect();
    let mut blocks = vec![
        toon::header(BIN, "Crew layout templates (structure only; the crew spec seats the agents)"),
        toon::list("layouts", &["name", "source", "slots", "dev_slots", "default", "summary"], &rows),
    ];
    for (t, _) in &all {
        blocks.push(format!("{}:\n{}", t.name, templates::diagram(t, &slot_labels(t))));
    }
    blocks.push(toon::kv("user_dir", &templates::user_dir().to_string_lossy()));
    blocks.push(toon::help(&[
        "Run `cmux-axi provision <project> --layout <name>`".to_string(),
        "Run `cmux-axi layout show <name>` for the JSON".to_string(),
    ]));
    println!("{}", toon::join(&blocks));
    Ok(())
}

pub fn layout_show(name: &str, json: bool) -> Result<()> {
    let t = templates::resolve(name)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&json!({
            "template": t, "slots": templates::slots(&t), "leaf_order": templates::leaf_order(&t),
            "tree": templates::compile(&t, (0..templates::slots(&t)).map(|_| templates::bare()).collect()),
        })).unwrap());
        return Ok(());
    }
    let blocks = vec![
        toon::header(BIN, &format!("layout {} — {}", t.name, t.summary)),
        templates::diagram(&t, &slot_labels(&t)),
        toon::kv("json", &format!("\n{}", serde_json::to_string_pretty(&t).unwrap_or_default())),
    ];
    println!("{}", toon::join(&blocks));
    Ok(())
}

/// Inputs for `layout create` (see `main` for the flags).
#[derive(Default)]
pub struct CreateOpts {
    pub rows: Option<String>,
    pub heights: Option<String>,
    pub widths: Option<String>,
    pub from_file: Option<String>,
    pub from_workspace: Option<String>,
    pub dev_slots: Option<String>,
    pub seats: Option<String>,
    pub summary: Option<String>,
    pub dir: Option<String>,
    pub force: bool,
}

fn layouts_dir(dir: Option<&str>) -> PathBuf {
    dir.map(PathBuf::from).unwrap_or_else(templates::user_dir)
}

pub fn layout_create(name: &str, o: &CreateOpts, json: bool) -> Result<()> {
    let sources = [o.rows.is_some(), o.from_file.is_some(), o.from_workspace.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if sources != 1 {
        return Err(CmuxError::usage("give exactly one of --rows <a,b,…>, --from-file <path>, --from-workspace <ref>"));
    }
    let mut t = if let Some(rows) = &o.rows {
        let rows = templates::parse_rows(rows, o.heights.as_deref(), o.widths.as_deref())?;
        templates::Template { name: name.to_string(), summary: String::new(), rows: Some(rows), tree: None, dev_slots: vec![], default_seats: Default::default() }
    } else if let Some(path) = &o.from_file {
        let mut t = templates::load_file(Path::new(path))?;
        t.name = name.to_string();
        t
    } else {
        let ws = o.from_workspace.as_deref().unwrap_or_default();
        cmux::ensure_installed()?;
        let list = cmux::list_panes(ws)?;
        let c = list.container_frame.unwrap_or_default();
        let frames: Vec<templates::Frame> = list
            .panes
            .iter()
            .filter_map(|p| p.pixel_frame.map(|f| templates::Frame { x: f.x, y: f.y, w: f.width, h: f.height }))
            .collect();
        let rows = templates::rows_from_frames(c.width, c.height, &frames).map_err(|e| {
            let frames_txt: Vec<String> = frames.iter().map(|f| format!("{}x{}@{},{}", f.w, f.h, f.x, f.y)).collect();
            CmuxError::usage(format!("{} — frames: container {}x{}; panes {}", e.message, c.width, c.height, frames_txt.join(" ")))
        })?;
        templates::Template { name: name.to_string(), summary: format!("captured from {ws}"), rows: Some(rows), tree: None, dev_slots: vec![], default_seats: Default::default() }
    };
    // Completions / overrides, valid for every source.
    if let Some(d) = &o.dev_slots {
        t.dev_slots = templates::parse_slots("--dev-slots", d)?;
    } else if t.dev_slots.is_empty() {
        if let Some(rows) = &t.rows {
            let n = templates::slots(&t);
            let last = rows.last().map(|r| r.panes).unwrap_or(0);
            t.dev_slots = (n - last..n).collect(); // the last row
        }
    }
    if let Some(seats) = &o.seats {
        t.default_seats = templates::parse_seats(seats)?;
    }
    if let Some(sm) = &o.summary {
        t.summary = sm.clone();
    }
    let path = templates::write_user(&layouts_dir(o.dir.as_deref()), &t, o.force)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&json!({"ok": true, "action": "layout-create", "path": path, "template": t})).unwrap());
        return Ok(());
    }
    println!(
        "{}",
        toon::join(&[
            toon::header(BIN, &format!("layout {} written", t.name)),
            toon::kv("path", &path.to_string_lossy()),
            templates::diagram(&t, &slot_labels(&t)),
            toon::help(&[format!("Run `cmux-axi provision <project> --layout {}`", t.name)]),
        ])
    );
    Ok(())
}

pub fn layout_rm(name: &str, force: bool, dir: Option<&str>, json: bool) -> Result<()> {
    let dir = layouts_dir(dir);
    let path = dir.join(format!("{name}.json"));
    if path.exists() && !force {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return Err(CmuxError::usage(format!("remove {}? pass --force (no terminal to confirm on)", path.display())));
        }
        eprint!("remove {}? [y/N] ", path.display());
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            println!("{}", toon::kv("cancelled", "true"));
            return Ok(());
        }
    }
    let removed = templates::remove_user(&dir, name)?;
    if json {
        println!("{}", serde_json::to_string(&json!({"ok": true, "action": "layout-rm", "removed": removed, "already": removed.is_none()})).unwrap());
    } else if let Some(p) = removed {
        println!("ok: layout rm {name} ({})", p.display());
    } else {
        println!("already: true (no user layout {name} in {})", dir.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared fleet rendering
// ---------------------------------------------------------------------------

fn print_fleet(entries: Vec<&FleetEntry>, json: bool) {
    if json {
        let arr: Vec<_> = entries
            .iter()
            .map(|e| {
                json!({
                    "role": e.role, "project": e.project, "surface": e.surface,
                    "session": e.session, "status": e.status,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"ok": true, "fleet": arr})).unwrap()
        );
        return;
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                e.role.clone(),
                e.project.clone(),
                e.surface.clone(),
                e.session.clone(),
                e.status.clone(),
            ]
        })
        .collect();
    println!(
        "{}",
        toon::join(&[
            toon::header(BIN, DESCRIPTION),
            toon::list(
                "fleet",
                &["role", "project", "surface", "session", "status"],
                &rows
            ),
        ])
    );
}
