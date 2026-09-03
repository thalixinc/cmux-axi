//! Thin typed wrapper over the `cmux` CLI — subprocess execution + JSON
//! introspection. Single owner of every `cmux` invocation in this crate.

use crate::error::{CmuxError, Result};
use serde::Deserialize;
use std::process::Command;

/// Run `cmux <args...>`, returning trimmed stdout. Non-zero exit maps to an
/// operational error carrying the first line of stderr.
pub fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("cmux")
        .args(args)
        .output()
        .map_err(|e| CmuxError::operational(format!("cmux not runnable: {e}"), "CMUX_EXEC"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let first = stderr.lines().next().unwrap_or("unknown cmux error");
        return Err(CmuxError::operational(
            format!("cmux {} failed: {first}", args.join(" ")),
            "CMUX_FAILED",
        ));
    }
    Ok(stdout)
}

/// Run `cmux <args...> --json` and deserialize the result.
pub fn run_json<T: for<'de> Deserialize<'de>>(args: &[&str]) -> Result<T> {
    let mut full = args.to_vec();
    full.push("--json");
    let text = run(&full)?;
    serde_json::from_str(&text).map_err(|e| {
        CmuxError::operational(
            format!("could not parse cmux --json output: {e}"),
            "CMUX_PARSE",
        )
    })
}

/// Resolve whether the `cmux` binary exists, with a helpful error otherwise.
pub fn ensure_installed() -> Result<()> {
    match Command::new("cmux").arg("--version").output() {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err(CmuxError::operational(
            "cmux is not installed or not on PATH",
            "CMUX_NOT_INSTALLED",
        )
        .with_suggestions(vec![
            "Install cmux and ensure `cmux --version` works.".into()
        ])),
    }
}

// ---------------------------------------------------------------------------
// Introspection models (serde)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
pub struct WorkspaceList {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
}

#[derive(Deserialize, Debug)]
pub struct Workspace {
    pub r#ref: String,
    #[serde(default)]
    pub custom_title: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct PanesList {
    #[serde(default)]
    pub panes: Vec<Pane>,
    /// Zero-sized unless the workspace is focused.
    #[serde(default)]
    pub container_frame: Option<ContainerFrame>,
}

#[derive(Deserialize, Debug, Clone, Copy, Default)]
pub struct ContainerFrame {
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
}

#[derive(Deserialize, Debug)]
pub struct Pane {
    pub r#ref: String,
    /// cmux's pane index — layout tree leaf order; the only stable geometry signal
    /// for an unfocused workspace (its pixel frames are all zero).
    #[serde(default)]
    pub index: Option<i64>,
    #[serde(default)]
    pub surface_refs: Vec<String>,
    #[serde(default)]
    pub pixel_frame: Option<PixelFrame>,
}

/// Only populated for the focused workspace.
#[derive(Deserialize, Debug, Clone, Copy)]
pub struct PixelFrame {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
}

#[derive(Deserialize, Debug)]
pub struct PaneSurfaces {
    #[serde(default)]
    pub surfaces: Vec<Surface>,
}

#[derive(Deserialize, Debug)]
pub struct Surface {
    pub r#ref: String,
    #[serde(default)]
    pub index: Option<i64>,
}

/// List all workspaces in the caller's window.
pub fn list_workspaces() -> Result<WorkspaceList> {
    run_json(&["workspace", "list"])
}

/// List panes in a workspace.
pub fn list_panes(workspace: &str) -> Result<PanesList> {
    run_json(&["list-panes", "--workspace", workspace])
}

/// List surfaces in a pane (scoped to its workspace — required, since a bare
/// `--pane` ref otherwise resolves against the caller's workspace).
pub fn list_pane_surfaces(pane: &str, workspace: &str) -> Result<PaneSurfaces> {
    run_json(&[
        "list-pane-surfaces",
        "--workspace",
        workspace,
        "--pane",
        pane,
    ])
}

/// Find a workspace by exact custom title.
pub fn find_workspace_by_title(title: &str) -> Result<Workspace> {
    let ws = list_workspaces()?;
    ws.workspaces
        .into_iter()
        .find(|w| w.custom_title.as_deref() == Some(title))
        .ok_or_else(|| {
            CmuxError::operational(
                format!("no workspace titled {title:?}"),
                "WORKSPACE_NOT_FOUND",
            )
        })
}
