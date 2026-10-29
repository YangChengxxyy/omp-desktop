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

Enter sends; Shift+Enter inserts a newline. While the agent is working, a new message is queued as a follow-up and the button switches to Stop (abort).

## What the basic version does

- Streams replies from delta updates; `message_end` is authoritative.
- Renders Markdown text, collapsible thinking, tool-call cards (arguments shown while they stream), and tool results.
- Answers omp dialogs: tool approval (`select`), `confirm`, `input` (masked when secret), `editor`, and `ask` (single/multi select, recommended option, free text).
- Status bar: connection state, cwd, model, thinking level, context usage, session cost.
- Startup errors (omp missing, too old, no protocol v2) and process exits show a reconnect action.

Not yet: session list and tabs, image paste, slash-command completion, diff rendering for `edit`, subagent panel. See `docs/tech-plan.md` §5.

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

The ignored UI tests open the real `ChatView` in a headless window, type a prompt, wait for the streamed reply, and drive a tool-approval dialog under `--approval-mode always-ask`.
