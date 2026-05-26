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
behind the `simulator` Cargo feature (on by default in `booth-bin`, and
shipped in the published `.deb`). The runtime is spawned with
`start_debug: false`, `listen_signals: false`, and `notify_systemd: false`
so the TUI owns Ctrl+C and the screen, and so the embedded debug HTTP/TLS
surface does not contend for ports. It is also spawned with
`runtime_mode: RuntimeMode::Simulator`, which means every `SystemSnapshot`
the booth pushes to the operator carries `runtimeMode: "simulator"` and the
operator UI renders a `SIM` badge next to the booth's status — even when
the simulator is paired with the real `booth-pi` audio + operator adapters.
Plain `--mock` runs report `runtimeMode: "mock"` the same way.

The terminal is set up via `ratatui` + `crossterm` and is restored from a
`Drop` guard so a panic or fatal error cannot leave the terminal in raw
mode.

## Running on the Raspberry Pi

The published `.deb` is built with `--features pi,systemd,simulator,mock`,
so `--simulator` and `--mock` work on an installed Pi just as they do in
development. Two ways to use them:

**Interactive — over SSH:**

```sh
sudo systemctl stop telephone-booth          # release GPIO + audio
sudo -u phonebooth /usr/bin/telephone-booth run --simulator [--mock]
```

The TUI runs in your SSH session. Quit with `q` to leave the GPIO/audio
pins free for the systemd unit to reclaim on restart.

**Autostart — via config:**

Both modes can be flipped on in `/etc/phone-booth/config.toml` so the
systemd unit picks them up without editing `ExecStart`:

```toml
[runtime]
mock      = true      # use mock HAL adapters (no rotary phone needed)
simulator = false     # see TTY caveat below before enabling
```

CLI flags can force a mode **on** at launch but cannot force it off —
the config file is the autostart source of truth. See
[`docs/configuration.md`](configuration.md#runtime-startup-mode) for
the full table.

`runtime.simulator = true` requires a TTY for the TUI; the stock
`telephone-booth.service` unit does **not** allocate one, so autostarting
in simulator mode needs a systemd override that points the service at a
console (`TTYPath=/dev/tty1` + `StandardInput=tty` +
`StandardOutput=tty`). `runtime.mock = true` has no such requirement and
works under the stock unit — handy for bringing up a Pi without the
rotary phone wired in.
