//! Command orchestration: provision, status, send, read, dev add/rm, teardown.
//! Every op prints TOON (default) or `--json` and returns `Result<()>`.

use crate::cmux;
use crate::error::{CmuxError, Result};
use crate::fleet::{self, FleetEntry};
use crate::layout;
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
fn state_root(cwd: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(p) => absolute(p),
        None => Ok(absolute(cwd)?.join(".omp").join("state")),
    }
}

// ---------------------------------------------------------------------------
// provision
// ---------------------------------------------------------------------------

pub fn provision(
    project: &str,
    cwd: &Path,
    harness: &str,
    devs: usize,
    state_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    cmux::ensure_installed()?;
    let cwd_abs = absolute(cwd)?;
    let state = state_root(&cwd_abs, state_dir)?;
    std::fs::create_dir_all(&state).map_err(|e| {
        CmuxError::operational(format!("cannot create {}: {e}", state.display()), "STATE")
    })?;
    let state_str = state.to_string_lossy().to_string();

    let title = ws_title(project);
    if let Ok(existing) = cmux::find_workspace_by_title(&title) {
        // Idempotent: already provisioned.
        let rows = fleet::load(&state.join("fleet.md"))?;
        print_fleet(rows.iter().filter(|e| e.project == project).collect(), json);
        println!(
            "{}",
            toon::kv("already", &format!("{} ({})", title, existing.r#ref))
        );
        return Ok(());
    }

    let spec = layout::CrewSpec {
        state_root: &state_str,
        harness,
        devs,
    };
    let layout_json = serde_json::to_string(&layout::build(&spec))
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

    let entries = map_roles_to_surfaces(&ws.r#ref, project, &state_str, harness, devs)?;
    let fleet_path = state.join("fleet.md");
    let mut all = fleet::load(&fleet_path)?;
    for e in &entries {
        fleet::upsert(&mut all, e.clone());
    }
    fleet::write(&fleet_path, &all)?;

    print_fleet(entries.iter().collect(), json);
    if !json {
        let help = vec![
            format!("Run `cmux-axi send {project} planner \"…\"` to steer"),
            "Run `cmux-axi status` for drift".to_string(),
        ];
        println!("\n{}", toon::help(&help));
    }
    Ok(())
}

/// Map the freshly-provisioned quad's panes/surfaces to roles using spatial
/// position (pixel_frame), and build fleet entries.
fn map_roles_to_surfaces(
    workspace: &str,
    project: &str,
    state_str: &str,
    harness: &str,
    devs: usize,
) -> Result<Vec<FleetEntry>> {
    let mut panes = cmux::list_panes(workspace)?.panes;
    // Sort spatially: top-left, top-right, bottom-left, bottom-right.
    panes.sort_by(|a, b| {
        let ay = a.pixel_frame.as_ref().map(|p| p.y).unwrap_or(0.0);
        let ax = a.pixel_frame.as_ref().map(|p| p.x).unwrap_or(0.0);
        let by = b.pixel_frame.as_ref().map(|p| p.y).unwrap_or(0.0);
        let bx = b.pixel_frame.as_ref().map(|p| p.x).unwrap_or(0.0);
        ay.partial_cmp(&by)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal))
    });

    let surfaces_of = |pane: &cmux::Pane| -> Result<Vec<cmux::Surface>> {
        Ok(cmux::list_pane_surfaces(&pane.r#ref, workspace)?.surfaces)
    };

    let started = now();
    let mut entries: Vec<FleetEntry> = Vec::new();
    let mut push = |role: &str, surface: &str, session: String| {
        entries.push(FleetEntry {
            role: role.to_string(),
            project: project.to_string(),
            surface: surface.to_string(),
            session,
            status: "active".to_string(),
            started: started.clone(),
        });
    };

    // panes[0]=top-left, [1]=top-right, [2]=bottom-left, [3]=bottom-right.
    if panes.len() != 4 {
        return Err(CmuxError::operational(
            format!("expected 4 panes in {workspace}, found {}", panes.len()),
            "LAYOUT_UNEXPECTED",
        ));
    }

    // Top-left: coordinator (surface 0), planner (surface 1).
    let tl = surfaces_of(&panes[0])?;
    if tl.len() < 2 {
        return Err(CmuxError::operational(
            "top-left pane missing coordinator/planner",
            "LAYOUT_UNEXPECTED",
        ));
    }
    push(
        "coordinator",
        &tl[0].r#ref,
        session_id(state_str, "coordinator", harness),
    );
    push(
        "planner",
        &tl[1].r#ref,
        session_id(state_str, "planner", harness),
    );

    // Top-right: brainstorm.
    let tr = surfaces_of(&panes[1])?;
    if tr.is_empty() {
        return Err(CmuxError::operational(
            "top-right pane missing brainstorm",
            "LAYOUT_UNEXPECTED",
        ));
    }
    push(
        "brainstorm",
        &tr[0].r#ref,
        session_id(state_str, "brainstorm", harness),
    );

    // Bottom panes: developers, round-robin (left=odd, right=even). Placeholder
    // surfaces beyond `devs` are skipped.
    let bl = surfaces_of(&panes[2])?;
    let br = surfaces_of(&panes[3])?;
    for (i, s) in bl.iter().enumerate() {
        let dev_id = 2 * i + 1;
        if dev_id <= devs {
            push(&format!("dev-{dev_id}"), &s.r#ref, "ephemeral".to_string());
        }
    }
    for (i, s) in br.iter().enumerate() {
        let dev_id = 2 * i + 2;
        if dev_id <= devs {
            push(&format!("dev-{dev_id}"), &s.r#ref, "ephemeral".to_string());
        }
    }

    Ok(entries)
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
    let rows = fleet::load(&state.join("fleet.md"))?;

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
    fleet::find(&rows, project, role)
        .map(|e| e.surface.clone())
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
    let ws = resolve_workspace(project)?;
    cmux::run(&["send", "--workspace", &ws, "--surface", &surface, text])?;
    cmux::run(&[
        "send-key",
        "--workspace",
        &ws,
        "--surface",
        &surface,
        "enter",
    ])?;
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

    // Pick the bottom pane with fewer surfaces (balance).
    let mut panes = cmux::list_panes(&ws.r#ref)?.panes;
    panes.sort_by(|a, b| {
        let ay = a.pixel_frame.as_ref().map(|p| p.y).unwrap_or(0.0);
        let by = b.pixel_frame.as_ref().map(|p| p.y).unwrap_or(0.0);
        ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
    });
    if panes.len() < 4 {
        return Err(CmuxError::operational(
            "crew quad not fully provisioned",
            "LAYOUT_UNEXPECTED",
        ));
    }
    let bottom = &panes[panes.len() - 2..];
    let pane = bottom
        .iter()
        .min_by_key(|p| p.surface_refs.len())
        .ok_or_else(|| CmuxError::operational("no bottom pane available", "LAYOUT_UNEXPECTED"))?;

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

    let mut cmd = match harness {
        "omp" => "omp --no-session".to_string(),
        other => other.to_string(),
    };
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
    // Mint a unique dev id if none given.
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

    if json {
        println!("{}", serde_json::to_string(&json!({"ok": true, "action": "teardown", "project": project, "workspace": ws.r#ref})).unwrap());
    } else {
        println!("ok: teardown {project} (closed {})", ws.r#ref);
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
