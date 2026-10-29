# omp-desktop

A desktop client for [omp](https://github.com/can1357/oh-my-pi), written in Rust with GPUI + gpui-kit. The agent itself runs in a local `omp --mode rpc-ui` child process; this app only provides the interface.

## Requirements

- omp ≥ 18.13.3 on `PATH` (`brew install can1357/tap/omp`)
- Rust 1.88+ (edition 2024)

## Run

```bash
cargo run -p omp-desktop                      # session in the current directory
cargo run -p omp-desktop -- --cwd ~/work/repo # session in another directory
```

Enter sends; Shift+Enter inserts a newline. While the agent is working, Enter queues a follow-up and ⌘Enter queues a steer (injected at the next tool boundary). Queued messages appear as chips above the composer: × withdraws one, 插队 promotes a follow-up to a steer. Esc in an empty composer, or the Stop button, aborts the run.

## What the basic version does

- Streams replies from delta updates; `message_end` is authoritative.
- Renders Markdown text and collapsible thinking. Each tool call is one card: `bash` streams its output with elapsed time, `edit` shows a line-numbered diff with +/- counts, `read` collapses to a one-line summary; failures are red and long output folds.
- Answers omp dialogs: tool approval (`select`), `confirm`, `input` (masked when secret), `editor`, and `ask` (single/multi select, recommended option, free text).
- Status bar: connection state, cwd, model, thinking level, context usage, session cost.
- Startup errors (omp missing, too old, no protocol v2) and process exits show a reconnect action.

Not yet: session list and tabs, image paste, slash-command completion, model/thinking switching, subagent panel. See `docs/tech-plan.md` §5.

## Layout

| Path | Role |
| --- | --- |
| `vendor/omp-rpc` | Official RPC client crate, pinned to omp tag `v18.13.3` (`UPSTREAM`) |
| `crates/omp-bridge` | Spawns omp, checks version/protocol, enables delta updates and the ask dialog, exposes an async event stream |
| `crates/omp-store` | Rebuilds messages from events; projects them to renderable blocks |
| `crates/omp-desktop` | The GPUI app |
| `docs/` | Technical plan and UI design (`docs/design/`) |

## Tests

```bash
cargo test --workspace                                   # offline unit tests
cargo test -p omp-desktop -- --ignored --test-threads 1  # headless UI against real omp (calls a model)
cargo run -p omp-bridge --example stream                 # CLI check: delta rebuild == message_end
```

The ignored UI tests open the real `ChatView` in a headless window against real omp: a streamed reply; a one-time tool approval under `--approval-mode always-ask`; an `ask` dialog answered with two options plus free text; a follow-up queued while `sleep 60` runs, withdrawn from its chip, then Esc aborting the run; and an `edit` whose card diff parses to +2/-2.
