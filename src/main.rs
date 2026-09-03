//! cmux-axi — AXI-compliant wrapper over `cmux` for AI agents.
//!
//! Command-first dispatch, TOON-default output with `--json` opt-in, idempotent
//! mutations, and 0/1/2 exit codes — the AXI family contract.

mod cmux;
mod crew;
mod error;
mod fleet;
mod layout;
mod ops;
mod setup;
mod templates;
mod toon;
mod version;

use error::{CmuxError, Result};
use std::collections::HashMap;

const BIN: &str = "cmux-axi";

/// Parsed argv: positionals + named flags (`--flag value` / `--flag=value` /
/// bare boolean `--flag`).
struct Parsed {
    positionals: Vec<String>,
    flags: HashMap<String, Option<String>>,
}

impl Parsed {
    fn parse(args: &[String]) -> Parsed {
        let mut positionals = Vec::new();
        let mut flags: HashMap<String, Option<String>> = HashMap::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(rest) = a.strip_prefix("--") {
                if let Some((name, value)) = rest.split_once('=') {
                    flags.insert(name.to_string(), Some(value.to_string()));
                } else if i + 1 < args.len() && takes_value(rest) {
                    flags.insert(rest.to_string(), Some(args[i + 1].clone()));
                    i += 1;
                } else {
                    flags.insert(rest.to_string(), None);
                }
            } else if a == "-v" || a == "-V" || a == "-h" {
                flags.insert(a.clone(), None);
            } else {
                positionals.push(a.clone());
            }
            i += 1;
        }
        Parsed { positionals, flags }
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn value(&self, name: &str) -> Option<String> {
        self.flags.get(name).cloned().flatten()
    }
}

/// Flags that consume the following token as a value.
fn takes_value(name: &str) -> bool {
    matches!(
        name,
        "devs" | "cwd" | "harness" | "state-dir" | "project" | "specialty" | "id" | "seed-prompt"
            | "layout"
    )
}

fn help() -> String {
    format!(
        "usage: {BIN} [command] [args] [flags]\n\
         commands[11]:\n\
         \x20 (none)=status dashboard, provision, status, send, read, dev, layout, teardown, setup, version, update\n\
         flags: --json (machine-readable), --state-dir <path>, --help, -v/-V/--version\n\
         examples:\n\
         \x20 {BIN} provision myproj --devs 2 --cwd ~/dev/myproj [--layout <name>]\n\
         \x20 {BIN} layout list | layout show 2by2\n\
         \x20 {BIN} status --project myproj\n\
         \x20 {BIN} send myproj planner \"plan the next epic\"\n\
         \x20 {BIN} read myproj coordinator\n\
         \x20 {BIN} dev add myproj --specialty node --seed-prompt brief.md\n\
         \x20 {BIN} dev rm myproj dev-1\n\
         \x20 {BIN} teardown myproj\n\
         \x20 {BIN} setup skill [--project] | setup hooks [--project]\n\
         \x20 {BIN} version | update [--check]"
    )
}

fn dispatch(args: &[String]) -> Result<()> {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", help());
        return Ok(());
    }
    if args
        .iter()
        .any(|a| a == "-v" || a == "-V" || a == "--version")
    {
        println!("{BIN} {}", version::VERSION);
        return Ok(());
    }

    let parsed = Parsed::parse(args);
    let cmd = parsed.positionals.first().map(String::as_str).unwrap_or("");
    let rest = &parsed.positionals[1..];
    let json = parsed.flag("json");
    let state_dir = parsed.value("state-dir").map(std::path::PathBuf::from);

    match cmd {
        "provision" => {
            let project = required(rest, 0, "project name")?;
            let cwd = parsed
                .value("cwd")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let harness = parsed.value("harness").unwrap_or_else(|| "omp".to_string());
            let devs = parsed
                .value("devs")
                .map(|d| d.parse().unwrap_or(2))
                .unwrap_or(2);
            let layout = parsed.value("layout").unwrap_or_else(|| templates::DEFAULT.to_string());
            ops::provision(project, &cwd, &harness, devs, &layout, state_dir.as_deref(), json)
        }
        "layout" => {
            let sub = rest.first().map(String::as_str).unwrap_or("");
            match sub {
                "list" => ops::layout_list(json),
                "show" => {
                    let name = required(&rest[1..], 0, "layout name")?;
                    ops::layout_show(name, json)
                }
                _ => Err(CmuxError::usage("unknown layout subcommand (list | show)")),
            }
        }
        "status" => {
            let project = parsed.value("project");
            ops::status(project.as_deref(), state_dir.as_deref(), json)
        }
        "send" => {
            let project = required(rest, 0, "project name")?;
            let role = required(rest, 1, "role or dev id")?;
            let text = rest.get(2).cloned().unwrap_or_default();
            ops::send(project, role, &text, state_dir.as_deref(), json)
        }
        "read" => {
            let project = required(rest, 0, "project name")?;
            let role = required(rest, 1, "role or dev id")?;
            ops::read(project, role, state_dir.as_deref(), json)
        }
        "dev" => {
            let sub = rest.first().map(String::as_str).unwrap_or("");
            match sub {
                "add" => {
                    let project = required(&rest[1..], 0, "project name")?;
                    let cwd = parsed
                        .value("cwd")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    let harness = parsed.value("harness").unwrap_or_else(|| "omp".to_string());
                    let specialty = parsed.value("specialty");
                    let id = parsed.value("id");
                    let seed = parsed.value("seed-prompt").map(std::path::PathBuf::from);
                    let worktree = parsed.flag("worktree") && !parsed.flag("no-worktree");
                    ops::dev_add(
                        project,
                        &cwd,
                        &harness,
                        specialty.as_deref(),
                        id.as_deref(),
                        seed.as_deref(),
                        worktree,
                        state_dir.as_deref(),
                        json,
                    )
                }
                "rm" => {
                    let project = required(&rest[1..], 0, "project name")?;
                    let dev_id = required(&rest[1..], 1, "dev id")?;
                    ops::dev_rm(
                        project,
                        dev_id,
                        parsed.flag("force"),
                        state_dir.as_deref(),
                        json,
                    )
                }
                _ => Err(CmuxError::usage("unknown dev subcommand (add | rm)")),
            }
        }
        "teardown" => {
            let project = required(rest, 0, "project name")?;
            ops::teardown(project, parsed.flag("force"), state_dir.as_deref(), json)
        }
        "version" => {
            version::cmd_version();
            Ok(())
        }
        "update" => version::cmd_update(parsed.flag("check"), json),
        "setup" => {
            let sub = rest.first().map(String::as_str).unwrap_or("");
            let global = !parsed.flag("project");
            match sub {
                "skill" => setup::cmd_setup_skill(global, json),
                "hooks" => setup::cmd_setup_hooks(global, json),
                _ => Err(CmuxError::usage("unknown setup subcommand (skill | hooks)")),
            }
        }
        other => Err(CmuxError::usage(format!(
            "unknown command {other:?} — run `{BIN} --help`"
        ))),
    }
}

fn required<'a>(args: &'a [String], idx: usize, what: &str) -> Result<&'a str> {
    args.get(idx)
        .map(String::as_str)
        .ok_or_else(|| CmuxError::usage(format!("missing {what}")))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = dispatch(&args) {
        eprintln!("{}", toon::error(&e.message, e.code, &e.suggestions));
        std::process::exit(e.exit_code());
    }
}
