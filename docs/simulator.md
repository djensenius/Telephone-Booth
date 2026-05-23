# Simulator TUI

The `telephone-booth` binary ships with an interactive terminal simulator that
drives the booth runtime through a mocked GPIO port. The state machine, audio
adapters, and operator HTTP client all run unchanged — only the rotary phone
hardware is synthesized from keyboard input.

This is the primary way to exercise the booth on a development machine
(macOS or Linux) without a rotary phone, dial board, or GPIO expander wired
up.

## Quick start

```sh
# Fully mocked: no audio device or operator backend required.
cargo run -p booth-bin -- run --simulator --mock

# Or via just:
just tui
```

To drive the real cross-platform audio + HTTP adapters (useful for verifying
playback/recording and operator integration end-to-end), drop `--mock`:

```sh
cargo run -p booth-bin -- run --simulator
```

In that mode the simulator uses the real `cpal`-backed playback and capture
plus the real `reqwest`-backed operator client. Make sure your config has a
working `[operator]` section — the simulator prints a warning at startup if
the base URL is still the example URL or the token is empty.

## Controls

| Key                  | Action                                                                 |
|----------------------|------------------------------------------------------------------------|
| `h` or `Space`       | Toggle the hook switch (on-hook ↔ off-hook).                            |
| `0` … `9`            | Dial a digit. A rotary "0" produces 10 pulses, otherwise N pulses.     |
| `q`, `Esc`, `Ctrl+C` | Send a Shutdown command to the runtime and exit the simulator.         |

Each digit press injects the correct number of falling+rising rotary pulse
edges, exactly as the GPIO adapter would observe them from a real dial. The
runtime's 350 ms pulse-group timeout decodes the pulse count into a digit.

To finish a recording cleanly: press `h` to hang up before pressing `q`.
Pressing `q` mid-call sends a Shutdown command immediately, which may cut
off an in-flight upload.

## UI layout

```text
╭─ Booth ─────────────────────────────────────────────────────────────╮
│ Telephone Booth Simulator   [mock I/O]   state=idle   status=idle   │
╰─────────────────────────────────────────────────────────────────────╯
╭─ Events (newest first) ─────────────────────────────────────────────╮
│ 2025-01-02T12:34:56  state -> dial_tone (cause: HookOff)            │
│ 2025-01-02T12:34:56  inject: hook level=false (Lifted receiver)     │
│ ...                                                                 │
╰─────────────────────────────────────────────────────────────────────╯
╭─ Audio In ──────────────╮ ╭─ Audio Out ─────────────╮
│ peak  0.12   rms  0.04  │ │ peak  0.40   rms  0.18  │
╰─────────────────────────╯ ╰─────────────────────────╯
╭─────────────────────────────────────────────────────────────────────╮
│ Controls: [h]/space toggle hook   [0-9] dial digit   [q] quit       │
│ Press [h] or space to lift the receiver.   Log: /tmp/telephone-...  │
╰─────────────────────────────────────────────────────────────────────╯
```

The events pane shows the most recent telemetry records (state transitions,
decoded digits, operator requests/responses, errors, log lines) newest at the
top. Audio gauges reflect the live level meter from the active sink/source.

## Logging

Standard tracing output would corrupt the TUI, so the simulator redirects all
tracing to a file:

- Default path: `/tmp/telephone-booth-sim.log`.
- Override via the `BOOTH_SIM_LOG_PATH` environment variable.

Tail it from another terminal:

```sh
tail -f /tmp/telephone-booth-sim.log
```

The current log path is shown in the footer of the TUI.

## How it differs from `--mock` (no `--simulator`)

| Mode                          | GPIO       | Audio      | Operator  | Interactive |
|-------------------------------|------------|------------|-----------|-------------|
| `run`                         | Pi (rppal) | Pi (cpal)  | Pi (HTTP) | no          |
| `run --mock`                  | Mock       | Mock       | Mock      | no          |
| `run --simulator`             | Mock       | Pi (cpal)  | Pi (HTTP) | TUI         |
| `run --simulator --mock`      | Mock       | Mock       | Mock      | TUI         |

`run --mock` (no simulator) just runs the headless event loop with mocks —
useful for integration tests but with no way to inject hook lifts or dial
pulses interactively.

## Implementation notes

The simulator lives in
[`crates/booth-bin/src/simulator.rs`](../crates/booth-bin/src/simulator.rs)
behind the `simulator` Cargo feature (on by default in `booth-bin`). The
runtime is spawned with `start_debug: false`, `listen_signals: false`, and
`notify_systemd: false` so the TUI owns Ctrl+C and the screen, and so the
embedded debug HTTP/TLS surface does not contend for ports.

The terminal is set up via `ratatui` + `crossterm` and is restored from a
`Drop` guard so a panic or fatal error cannot leave the terminal in raw
mode.
