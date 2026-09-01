# cmux-axi

AXI-compliant Rust CLI wrapping [`cmux`](https://cmux.com) for AI agents — the 7th member of the AXI tool family (gh-axi, tasks-axi, quota-axi, lavish-axi, chrome-devtools-axi, no-mistakes).

Make cmux operations cheap and deterministic for agents: token-efficient TOON output, combined/idempotent operations, single calls instead of raw `cmux` flag-memorization and `list-* | grep` gymnastics. Take as much off the AI as possible.

## What it does

Provisions the **crew layout** — three master roles (Coordinator, Brainstorm, Planner) plus N disposable developers — as a deterministic 2×2 quad of cmux surfaces, each running a harness in a uniquely-identified session:

```
┌──────────────────────────┬──────────────────────────┐
│ coordinator · planner    │ brainstorm               │
├──────────────────────────┼──────────────────────────┤
│ developers (tabs)        │ developers (tabs)        │
└──────────────────────────┴──────────────────────────┘
```

- **Masters** are resumable: each launches `omp --session-dir …/sessions/<role>` so a window can be reopened and resumed without colliding with a neighbor.
- **Developers** are disposable: launched with `omp --no-session`, torn down after their task.
- Every role → surface → session binding is recorded in a durable `fleet.md`.

## Install

```sh
cargo install --git https://github.com/thalixinc/cmux-axi
```

Requires `cmux` on `PATH` and (default) `omp` as the harness. Node 20+ is **not** required — this is a single static binary.

## Usage

```sh
cmux-axi                                    # dashboard: fleet map + next steps
cmux-axi provision myproj --devs 2 --cwd ~/dev/myproj
cmux-axi status --project myproj            # fleet map ⊕ live cmux, drift flagged
cmux-axi send myproj planner "plan the next epic"
cmux-axi read myproj coordinator            # read-screen on that surface
cmux-axi dev add myproj --specialty node --seed-prompt brief.md
cmux-axi dev rm myproj dev-1
cmux-axi teardown myproj
```

Flags: `--json` (machine-readable), `--harness omp|claude|codex` (default `omp`),
`--state-dir <path>` (default `<cwd>/.omp/state`), `--help`, `-v/--version`.

Output is TOON by default; `--json` is the explicit opt-in. Mutations are idempotent
(`already: true` on a no-op) and exit 0/1/2 (success / operational failure / usage).

## Wrapped command surface (cmux 0.64.22)

`new-workspace`/`workspace create` (`--cwd`/`--command`/`--layout`) · `list-panes` /
`list-pane-surfaces` / `workspace list` / `tree` · `new-surface` / `new-split` /
`close-surface` / `close-workspace` · `send` / `send-key` / `read-screen` / `identify`.

## Footguns this wrapper absorbs

- **Workspace targeting:** surface/pane ops default to the caller's workspace
  (`CMUX_WORKSPACE_ID`). Every op passes an explicit `--workspace`.
- **`send` doesn't auto-submit:** `cmux-axi send` chains `send-key enter` in one call.
- **Ref vs index addressing:** a bare number is an *index*, not the `surface:<n>` ref.
  The wrapper resolves refs internally and never emits a bare index.
- **`close-surface` ≠ split collapse:** teardown closes panes/workspaces, not bare surfaces.
- **Harness session persistence:** a second harness in the same directory can resume an
  existing session. Masters get a unique `--session-dir` per role; developers get
  `--no-session`.

## License

MIT
