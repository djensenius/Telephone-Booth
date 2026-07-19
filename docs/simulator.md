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

On Unix the simulator also re-points the process's `stderr` (FD 2) at the
same log file. That way C-level libraries like `alsa-lib` — which write
diagnostics directly to FD 2, bypassing `tracing` — also land in the log
instead of punching through the TUI buffer. If you see `ALSA lib pcm_*.c`
spew on the framebuffer, you're running an old build; pull the latest.

## How it differs from `--mock` (no `--simulator`)

| Mode                          | GPIO       | Audio      | Operator  | Interactive |
|-------------------------------|------------|------------|-----------|-------------|
| `run`                         | Pi (rppal) | Pi (cpal)  | Pi (HTTP) | no          |
| `run --mock`                  | Mock       | Mock       | Mock      | no          |
| `run --simulator`             | Mock       | Pi (cpal)  | Pi (HTTP) | TUI         |
| `run --simulator --mock`      | Mock       | Mock       | Mock      | TUI         |
| `run --tui`                   | Pi (rppal) | Pi (cpal)  | Pi (HTTP) | TUI (read-only) |
| `run --tui --mock`            | Mock       | Mock       | Mock      | TUI (read-only) |
| `run --tui --attach <url>`    | none       | none       | none      | TUI (attached, read-only) |

`run --mock` (no simulator) just runs the headless event loop with mocks —
useful for integration tests but with no way to inject hook lifts or dial
pulses interactively.

## Read-only hardware monitor (`--tui`)

`run --tui` launches the *same* TUI surface as the simulator, but wired to the
**real** HAL adapters instead of a mocked GPIO port. It injects nothing — you
dial the physical rotary phone and watch the live telemetry scroll by. This is
the on-Pi equivalent of "watching `journalctl -f`", but with the booth state,
decoded digits, audio meters, and operator calls laid out in a live dashboard.

```sh
# On the Pi, over SSH — monitor the real hardware:
sudo systemctl stop telephone-booth          # release GPIO + audio first!
sudo -u phonebooth /usr/bin/telephone-booth run --tui
```

Because the monitor drives the real `rppal` GPIO, `cpal` audio, and `reqwest`
operator adapters, it **reserves the same GPIO pins and audio device as the
`telephone-booth.service` unit**. The two cannot run at once — stop the service
before starting the monitor, and quit the monitor (`q`) before restarting the
service.

If you want the same full-screen terminal dashboard **without** stopping the
service, use attach mode instead:

```sh
# Local Pi: attach over the loopback + LAN debug listeners without opening GPIO/audio.
sudo -u phonebooth BOOTH_DEBUG_TOKEN=... /usr/bin/telephone-booth run --tui --attach https://127.0.0.1:8443

# Remote: attach over the Tailscale-served HTTPS endpoint.
telephone-booth run --tui --attach https://telephone-booth.<tailnet>.ts.net --token ...
```

Attach mode never builds local adapters. It only consumes the running service's
`/v1/ws/telemetry` stream, so GPIO pins, ALSA devices, and the operator client
stay owned by `telephone-booth.service`.

Only quit keys are active in monitor mode:

| Key                  | Action                                         |
|----------------------|------------------------------------------------|
| `q`, `Esc`, `Ctrl+C` | Shut the runtime down and exit the monitor.    |

Hook and dial keys are ignored (the header/footer say so) — there is no
injector, so events come only from the physical phone. The header reads
`Telephone Booth Monitor   [real I/O]` and the hook indicator follows the real
receiver via the hardware's hook GPIO edges.

`run --tui --mock` is also accepted (monitor the mock adapters), but that is
mostly a curiosity — `--simulator --mock` is the interactive equivalent.

### Monitor vs. web pin matrix

There are now three ways to watch a booth, depending on whether you need local
hardware ownership or a passive remote view:

| Surface | GPIO/audio opened by this process? | Control injection | Best for |
| ------- | ---------------------------------- | ----------------- | -------- |
| `run --tui` | yes | no | On-Pi bring-up when you can stop the service |
| `run --tui --attach <url>` | no | no | SSH/Tailscale dashboard of a running booth |
| `/v1/ui/simulator` | no | web controls only in `mock`/`simulator` runtime modes | Browser-based remote watch/debug |

Attach mode is strictly read-only. Only quit keys are active, just like the
local `--tui` monitor.

### TLS in attach mode

`--attach` accepts `https://` and `wss://` base URLs for remote hosts and always
connects to `/v1/ws/telemetry`. Plaintext `http://` and `ws://` are accepted
only for loopback hosts (`localhost`, `127.0.0.1`, or `::1`) so the bearer token
is never sent in cleartext over the network.

- **Remote Tailscale HTTPS** uses normal WebPKI validation.
- **Local loopback HTTPS** (`https://127.0.0.1:8443`) bootstraps the generated
  self-signed cert by fetching `/v1/cert/fingerprint` from the authenticated
  loopback HTTP listener (`[debug].loopback_bind`) and pinning that SHA-256
  fingerprint for the WebSocket connection.

The attach client does **not** blanket-disable TLS verification for arbitrary
hosts. If you are connecting to a non-Tailscale remote HTTPS endpoint with a
self-signed cert, trust that CA separately or attach through the loopback/Tailscale
path instead.

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

The read-only monitor (`run_monitor`) and attach mode (`run_attached`) share
the same `SimulatorState`, render loop, and terminal guard via the internal
`drive_tui` helper. Local monitor mode passes no `GpioInjector` and spawns the
runtime with `runtime_mode: RuntimeMode::Real` (or `Mock` with `--mock`);
attach mode skips runtime creation entirely and feeds the same render loop from
the debug WebSocket stream instead. In both cases, keypresses other than quit
are no-ops, and the hook indicator is driven from observed telemetry rather
than injected edges.

## Running on the Raspberry Pi

The published `.deb` is built with `--features pi,systemd,simulator,mock`,
so `--simulator` and `--mock` work on an installed Pi just as they do in
development. Three ways to use them:

**Interactive — over SSH (ad-hoc):**

```sh
sudo systemctl stop telephone-booth          # release GPIO + audio
sudo -u phonebooth /usr/bin/telephone-booth run --simulator [--mock]
```

The TUI runs in your SSH session. Quit with `q` to leave the GPIO/audio
pins free for the systemd unit to reclaim on restart.

**Persistent — tmux service (SSH-attachable):**

A separate `telephone-booth-simulator.service` unit runs the simulator
inside a tmux session. Switch between modes with the `telephone-booth-mode`
script:

```sh
# Switch to simulator mode
sudo telephone-booth-mode simulator

# Attach from any SSH session
just attach
# (long form: sudo tmux -S /run/telephone-booth/tmux.sock attach -t telephone-booth)
```

Detach with `Ctrl+B, D` — the booth keeps running. The tmux socket lives
at `/run/telephone-booth/tmux.sock` (owned by the `phonebooth` user via
`RuntimeDirectory`).

To switch back to the stock headless service:

```sh
sudo telephone-booth-mode headless
```

Check which mode is active (and whether anyone is currently attached to
the simulator tmux session):

```sh
sudo telephone-booth-mode status
# (or, from a developer checkout: just status)
```

**Browser — web UI controls:**

When the debug surface has `allow_controls = true`, a self-contained
simulator control page is served at:

```text
https://<your-tailscale-hostname>/v1/ui/simulator
```

In **simulator mode** (`telephone-booth-mode simulator` or `--simulator`),
the booth automatically:

- Starts the embedded debug surface (same Tailscale-served endpoint as the
  headless booth uses for read-only telemetry).
- Pre-enables `[debug] allow_controls` so the hook and dial buttons work
  out of the box — no config edit required.

The TUI and the web simulator share a single state machine and a single
`TelemetryBus`, so injecting a hook lift from the browser is reflected in
the TUI's event log in real time and vice versa. You can drive the booth
from either surface (or both) and the operator console stays in sync via
its usual `PUT /v1/status` + `PUT /v1/system` push path.

The page provides:

- Hook toggle (lift/hang up)
- Dial pad (0–9, with correct pulse counts)
- Live telemetry event stream via WebSocket
- Current state display

**Keyboard shortcuts** (active once you've authenticated and aren't typing
into a text field):

| Key                       | Action                          |
| ------------------------- | ------------------------------- |
| `0`–`9` (top row, numpad) | Dial that digit                 |
| `Space` or `H`            | Toggle hook (lift / hang up)    |
| `?`                       | Show a brief help toast         |

Authentication uses the same debug bearer token. Enter it in the page's
login form — the token is held only in browser memory and transmitted via
`Authorization` header (HTTP) and `Sec-WebSocket-Protocol: bearer.<token>`
(WebSocket). It never appears in URLs.

Enable controls in `/etc/phone-booth/config.toml` (only needed for the
`--mock` / headless-with-mocks flavour; simulator mode auto-enables this):

```toml
[debug]
allow_controls = true
```

**Controls are blocked against a real-hardware booth.** Even when
`allow_controls = true`, `/v1/simulate/event` and `/v1/simulate/pulse`
return `403` (with `{"reason": "headless_real_hardware"}`) whenever the
booth is running in `RuntimeMode::Real` — i.e. the regular
`telephone-booth.service` wired to actual GPIO, audio, and operator HTTP.
This prevents synthetic events from racing with hardware events on a live
booth. The web UI detects this via `/v1/config` and shows a "headless /
real-hardware mode" banner with the hook and dial buttons disabled; the
event stream and state display stay live. Controls are only accepted in
`RuntimeMode::Mock` or `RuntimeMode::Simulator` — see
[`crates/booth-hal/src/lib.rs`](../crates/booth-hal/src/lib.rs) for the
enum definition.

**Autostart — via config (headless, no TUI):**

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
in simulator mode needs either the tmux drop-in above or a custom systemd
override that points the service at a console (`TTYPath=/dev/tty1` +
`StandardInput=tty` + `StandardOutput=tty`). `runtime.mock = true` has no
such requirement and works under the stock unit — handy for bringing up a
Pi without the rotary phone wired in.
