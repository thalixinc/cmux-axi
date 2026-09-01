# cmux-axi

AXI-compliant Rust CLI wrapping [`cmux`](https://cmux.com) for AI agents — the 7th member of the AXI tool family (gh-axi, tasks-axi, quota-axi, lavish-axi, chrome-devtools-axi, no-mistakes).

Make cmux operations cheap and deterministic for agents: token-efficient TOON output, combined/idempotent operations, single calls instead of raw `cmux` flag-memorization and `list-* | grep` gymnastics. Take as much off the AI as possible.

---

## What we gained (why this wrapper exists)

cmux-axi does **not** make your harness do its coding work any faster or cheaper. `omp` (or claude/codex) spends the same tokens on the actual task either way. What it removes is the **orchestration tax** — the context tokens an agent burns just to *operate the multiplexer* rather than *do the work*.

The gain is threefold, and it's real:

### 1. Fewer input tokens (what the agent *reads*)

cmux's native introspection is verbose. On a window with a handful of workspaces:

| Command | Output size |
|---|---|
| `cmux workspace list --json` | ~15,800 bytes (~4,000 tokens) |
| `cmux tree --all` | ~3,600 bytes |
| **`cmux-axi status`** | **~200–400 bytes (~tens of tokens)** |

`cmux-axi status` returns the same fleet map as five compact CSV rows instead of a multi-KB JSON blob full of daemon/proxy/port/remote metadata the agent never needs. Every read-back is an order of magnitude cheaper.

### 2. Fewer output tokens and turns (what the agent *writes*)

Raw cmux has no "send a message to the Planner" — it has a recipe the agent must remember and reassemble every time:

```sh
# raw cmux: 5 commands + jq gymnastics
cmux workspace list --json | jq '…'          # resolve the workspace ref
cmux list-panes --workspace cf-proj --json | jq '…'        # resolve the pane
cmux list-pane-surfaces --workspace cf-proj --pane pane:N --json | jq '…'  # resolve the surface
cmux send --workspace cf-proj --surface surface:M "plan X"
cmux send-key --workspace cf-proj --surface surface:M enter
```

```sh
# cmux-axi: 1 command
cmux-axi send proj planner "plan X"
```

Combined operations (`send` auto-submits Enter; `provision` builds the whole quad in one call) collapse multi-command recipes into single deterministic calls. Fewer commands written = fewer output tokens, fewer turns, fewer chances to get a ref wrong.

### 3. Idempotency and footgun absorption (fewer retries)

Every mutation is idempotent — re-running a failed command reports `already: true` instead of double-creating. And the five footguns that make raw cmux error-prone for an agent (below) are encapsulated so the agent never has to reason about them.

**Net effect:** the agent spends its context budget on the task, not on operating the terminal multiplexer. Same harness, more headroom.

---

## Install

Prerequisites:

- [`cmux`](https://cmux.com) on your `PATH` (`cmux --version`).
- A harness: `omp` (default), or `claude`/`codex` via `--harness`.

Build from source (single static binary, no Node runtime required):

```sh
cargo install --git https://github.com/thalixinc/cmux-axi
```

Or build locally:

```sh
git clone https://github.com/thalixinc/cmux-axi
cd cmux-axi
cargo build --release
./target/release/cmux-axi --version
```

---

## Quick start

```sh
cmux-axi                                        # dashboard: fleet map + next steps
cmux-axi provision myproj --devs 2 --cwd ~/dev/myproj
cmux-axi status --project myproj                # fleet map ⊕ live cmux, drift flagged
cmux-axi send myproj planner "plan the next epic"
cmux-axi read myproj coordinator                # read-screen on that surface
cmux-axi dev add myproj --specialty node --seed-prompt brief.md
cmux-axi dev rm myproj dev-1
cmux-axi teardown myproj
```

`provision` creates one cmux workspace named `cf-myproj` containing the whole crew. `teardown` closes the workspace **and kills every harness process in it** (verified — no orphans).

---

## The crew layout

`provision` builds a fixed 2×2 **quad**:

```
┌──────────────────────────┬──────────────────────────┐
│ top-left pane            │ top-right pane           │
│   [Coordinator] [Planner]│   Brainstorm             │
├──────────────────────────┼──────────────────────────┤
│ bottom-left pane (devs)  │ bottom-right pane (devs) │
│   [dev-1]                │   [dev-2]                │
│   [dev-3] …              │   [dev-4] …              │
└──────────────────────────┴──────────────────────────┘
```

- **Coordinator** and **Planner** share the top-left pane as two tabs.
- **Brainstorm** owns the top-right pane.
- **Developers** fill the two bottom quadrants as tabs, round-robin: dev 1, 3, 5… → bottom-left; dev 2, 4, 6… → bottom-right. More developers = more tabs, not more panes.

`--devs N` sets the initial developer count (default 2). `--devs 0` provisions the quad with empty developer slots, ready for `dev add`.

### Session model

Each surface runs the harness with **role-scoped session isolation**, so a second harness in the same directory can never resume a neighbor's session:

| Role class | Launch command | Effect |
|---|---|---|
| Masters (coordinator, planner, brainstorm) | `omp --session-dir <state>/sessions/<role>` | **Resumable** — reopen the window and resume exactly that role's session |
| Developers | `omp --no-session` | **Ephemeral** — disposable, nothing to resume |

Session isolation is `omp`-specific today; `--harness claude` / `--harness codex` launch the binary bare.

---

## Command reference

Every command is `<cmux-axi> <command> ...args ...flags`. Bare `cmux-axi` (no command) is the dashboard.

### `provision <project>`

Create the crew quad for a project.

```
cmux-axi provision <project> [--devs N] [--cwd <path>] [--harness <h>] [--state-dir <path>] [--json]
```

- Creates a workspace named `cf-<project>` with the quad layout and launches the harness in every surface.
- **Idempotent** — re-running on an existing project reports `already: true` and prints the existing fleet instead of double-creating.
- `--devs N` — initial developers (default 2).
- `--cwd <path>` — the project directory the harness starts in (default `.`).

### `status`

Print the fleet map — every known role → surface → session — with drift flagged.

```
cmux-axi status [--project <project>] [--state-dir <path>] [--json]
```

With `--project`, scopes to one crew and reports whether it is currently provisioned.

### `send <project> <role|dev-id> <text>`

Type-and-submit to a role's surface (absorbs the "`send` doesn't auto-submit" footgun).

```
cmux-axi send <project> <role|dev-id> <text> [--state-dir <path>] [--json]
```

`<role|dev-id>` is `coordinator`, `brainstorm`, `planner`, or a developer id like `dev-1`.

### `read <project> <role|dev-id>`

Read the rendered screen of a role's surface.

```
cmux-axi read <project> <role|dev-id> [--state-dir <path>] [--json]
```

### `dev add <project>`

Spin up a disposable developer — a new tab in a developer pane running an ephemeral harness.

```
cmux-axi dev add <project> [--specialty <s>] [--id <id>] [--seed-prompt <path>]
                  [--worktree | --no-worktree] [--cwd <path>] [--harness <h>]
                  [--state-dir <path>] [--json]
```

- `--specialty <s>` — a recorded label (e.g. `node`, `python`); informational only.
- `--id <id>` — explicit developer id; otherwise a unique `dev-N` is minted.
- `--seed-prompt <path>` — file sent as the harness's first message (persona / brief / rules).
- `--worktree` — create an isolated `git worktree` (at `<cwd>/.omp/worktrees/<id>`, branch `cmux-axi/<id>`) and run the harness there.

### `dev rm <project> <dev-id>`

Tear down one developer.

```
cmux-axi dev rm <project> <dev-id> [--force] [--state-dir <path>] [--json]
```

Closes the developer's surface and removes its fleet record.

### `teardown <project>`

Remove the whole crew.

```
cmux-axi teardown <project> [--force] [--state-dir <path>] [--json]
```

Closes the `cf-<project>` workspace — panes, surfaces, and every harness process — and clears the project's fleet records. Already-torn-down projects report `already: true`.

---

## Flags

| Flag | Applies to | Meaning |
|---|---|---|
| `--json` | all | Machine-readable JSON instead of TOON |
| `--state-dir <path>` | all | State root (default `<cwd>/.omp/state`) |
| `--devs N` | provision | Initial developer count (default 2) |
| `--cwd <path>` | provision, dev add | Project directory (default `.`) |
| `--harness <h>` | provision, dev add | `omp` (default) \| `claude` \| `codex` |
| `--specialty <s>` | dev add | Recorded developer label |
| `--id <id>` | dev add | Explicit developer id |
| `--seed-prompt <path>` | dev add | File sent as the harness's first message |
| `--worktree` / `--no-worktree` | dev add | Isolated git worktree (default off) |
| `--force` | dev rm, teardown | Skip safety refusal |
| `--help`, `-v`/`-V`/`--version` | — | Help / version |

---

## Output contract

- **TOON is the default** — compact `label[count]{schema}:` rows plus a `help[N]:` block of next steps:

  ```
  bin: cmux-axi
  description: Provision the codefactory crew layout in cmux
  fleet[5]{role,project,surface,session,status}:
    coordinator,myproj,surface:109,…/sessions/coordinator,active
    planner,myproj,surface:110,…/sessions/planner,active
    brainstorm,myproj,surface:112,…/sessions/brainstorm,active
    dev-1,myproj,surface:111,ephemeral,active
    dev-2,myproj,surface:113,ephemeral,active
  help[1]:
    - Run `cmux-axi send myproj planner "…"` to steer
  ```

- **`--json`** is the explicit opt-in for machine-readable output.
- **Mutations** lead with a terse `ok:` line and report `already: true` on a no-op, so retries are safe.
- **Exit codes**: `0` success · `1` operational failure · `2` usage/validation error.
- **Errors** render as `error:` + `code:` + optional `help:` suggestions.

---

## State on disk

Everything is under the state root (default `<cwd>/.omp/state`; override with `--state-dir`):

| Path | Contents |
|---|---|
| `<state>/fleet.md` | The durable `role → surface → session` record (single writer: cmux-axi) |
| `<state>/sessions/<role>` | Master session dirs (resumable roles) |
| `<cwd>/.omp/worktrees/<id>` | Developer worktrees (`dev add --worktree`) |

`fleet.md` is the source of truth for `status`/`send`/`read`/`dev rm` — resolving "which surface is the Planner" is always a file read, never a memory.

---

## Footguns absorbed

Raw cmux has five behaviors that reliably trip up an agent. cmux-axi encapsulates all five:

| # | Footgun | cmux-axi behavior |
|---|---|---|
| 1 | Surface/pane ops default to the *caller's* workspace (`CMUX_WORKSPACE_ID`), not the one you mean | Every op passes an explicit `--workspace` |
| 2 | `send` does **not** auto-submit — text parks unsubmitted | `send` chains `send-key enter` in one call |
| 3 | A bare number is an *index*, not the `surface:<n>` ref | The wrapper resolves refs internally and never emits a bare index |
| 4 | `close-surface` ≠ collapsing the split — the pane respawns a terminal | `teardown` closes the workspace; `dev rm` targets the surface by ref |
| 5 | A second harness in the same directory can resume the existing session | Masters get a unique `--session-dir` per role; developers get `--no-session` |

---

## How it works

1. **Layout JSON** — `provision` builds the quad as a single nested `--layout` tree and hands it to `cmux new-workspace --layout …` in one call.
2. **Spatial role mapping** — after creation, cmux-axi introspects panes (`list-panes --json`) and sorts them by their `pixel_frame` (x, y) coordinates, so `top-left / top-right / bottom-left / bottom-right` is resolved from geometry, never from ordering assumptions.
3. **Fleet record** — the resulting `role → surface → session` bindings are written to `fleet.md` atomically (temp file + rename), then read back by every subsequent command.
4. **Subprocess calls** — every `cmux` invocation goes through one thin wrapper module (single owner), so compatibility and call-shape live in one place.

---

## Known limitations

- **Session isolation is `omp`-only.** `--harness claude` / `--harness codex` launch the binary bare; their own session-resume semantics are not yet scoped by cmux-axi.
- **`dev rm` uses `close-surface`, not `close-pane`.** The developer's tab closes, but the pane may respawn a bare terminal slot (the empty slot is where the next developer lands — harmless, but not a collapsed split).
- **The landed-work gate is not yet enforced.** `dev rm --force` is accepted; the "never tear down unlanded work" check (uncommitted/unpushed worktree) is a documented future guard, not yet wired.
- **`--force` is accepted but currently a no-op** on `teardown` and `dev rm` — the refuse-on-unlanded-work path it would gate does not exist yet.
- **Workspace grouping is deferred.** Projects provision as flat `cf-<project>` workspaces; per-project workspace groups are a follow-up.

---

## License

MIT
