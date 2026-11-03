# omp-desktop

A desktop client for [omp](https://github.com/can1357/oh-my-pi), written in Rust with GPUI + gpui-kit. The agent itself runs in a local `omp --mode rpc-ui` child process; this app only provides the interface.

## Requirements

- omp ≥ 18.13.3 on `PATH` (`brew install can1357/tap/omp`)
- Rust 1.88+ (edition 2024)

## Run

```bash
cargo run -p omp-desktop                      # session in the current directory
cargo run -p omp-desktop -- --cwd ~/work/repo # session in another directory
cargo run -p omp-desktop -- --session-dir /tmp/s # store and list sessions there
```

Each tab runs its own omp process. The sidebar lists saved sessions from `~/.omp/agent/sessions` grouped by directory, including ones started in the TUI; click one to resume it, 分支 to fork it into a new file.

## Shortcuts

| Keys | Action |
| --- | --- |
| ⌘N / ⌘W | New tab / close tab (stops its omp process) |
| ⌘1–9 | Switch tab |
| ⌘P | Search saved sessions |
| ⌘B | Toggle the session sidebar |
| ⌘J | Toggle the status panel (context, cost, todos, subagents) |
| ⌘K | Command palette: actions, models, thinking levels, slash commands, theme |
| ⌘M | Pick a model for this tab |
| ⌘T | Cycle the thinking level |
| ⌘⇧L | Accounts: log in to or out of a provider |
| Enter / Shift+Enter | Send / newline |
| Enter while running | Queue a follow-up |
| ⌘Enter while running | Queue a steer (injected at the next tool boundary) |
| Esc | Abort the run (empty composer); closes popups and panels first |
| `/` at line start | Slash-command completion: ↑↓ select, Tab/Enter complete, Esc dismiss |
| ⌘V / drag files | Attach images |

Queued messages appear as chips above the composer: × withdraws one, 插队 promotes a follow-up to a steer.

## What it does

- Streams replies from delta updates; `message_end` is authoritative.
- Renders Markdown text and collapsible thinking. Each tool call is one card: `bash` streams its output with elapsed time, `edit` shows a line-numbered diff with +/- counts, `read` collapses to a one-line summary; failures are red and long output folds.
- Answers omp dialogs: tool approval (`select`), `confirm`, `input` (masked when secret), `editor`, and `ask` (single/multi select, recommended option, free text).
- Status bar: connection state, cwd, model and thinking menus, context usage, compaction, session cost; ⌘J opens the full state and the subagent panel (steer or cancel a subagent).
- Slash commands from omp's catalog with aliases, subcommands, and argument hints; builtin command output shows inline.
- Accounts panel (title bar 账号, ⌘⇧L): search omp's login providers, OAuth login (opens the browser, copyable link, paste-the-code fallback, cancel), and logout per stored credential. It runs its own omp process, so it works even when no session can start; a tab that failed for missing credentials offers it and reconnects after a login.
- Theme: follow the system (live) or force light/dark from the title bar 主题 menu or ⌘K; the choice is saved in `~/Library/Application Support/omp-desktop/theme`.
- Startup errors (omp missing, too old, no protocol v2, no usable model) and process exits show a reconnect action; a session already on disk reconnects with `--resume`.

## Layout

| Path | Role |
| --- | --- |
| `vendor/omp-rpc` | Official RPC client crate, pinned to omp tag `v18.13.3` (`UPSTREAM`) |
| `crates/omp-bridge` | Spawns omp, checks version/protocol, enables delta updates and the ask dialog, exposes an async event stream |
| `crates/omp-store` | Rebuilds messages from events; projects them to renderable blocks; lists saved sessions |
| `crates/omp-desktop` | The GPUI app |
| `docs/` | Technical plan and UI design (`docs/design/`) |

## Tests

```bash
cargo test --workspace                                   # offline unit tests
cargo test -p omp-desktop -- --ignored --test-threads 1  # headless UI against real omp (calls a model)
cargo run -p omp-bridge --example stream                 # CLI check: delta rebuild == message_end
cargo run -p omp-store --example sessions                # print the session list the sidebar shows
```

The ignored UI tests run the real views in a headless window against real omp: a streamed reply; a one-time tool approval under `--approval-mode always-ask`; an `ask` dialog answered with two options plus free text; a follow-up queued while `sleep 60` runs, withdrawn from its chip, then Esc aborting the run; an `edit` whose card diff parses to +2/-2; ⌘Enter queuing a steer; a chip promoting a follow-up to a steer; confirm/input/editor dialogs opened by the fixture extension `tests/fixtures/dialogs.ts`; tab shortcuts; slash completion and a builtin command; the ⌘K palette, ⌘M, and ⌘T; the accounts panel in a scratch `PI_CODING_AGENT_DIR` (login up to the browser link and cancel, logout of a fake key); the no-credentials screen; and the M3 acceptance run: a session made by `omp -p` is listed, resumed with its history, continued, reconnected after `kill -9`, and forked.
