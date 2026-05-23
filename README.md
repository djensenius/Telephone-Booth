# Telephone Booth — phone client (Rust)

> Pick up the receiver. Dial 1 to hear a question. Speak after the beep.
> Dial 2 to listen to someone else's message. Hang up to leave.

A Rust rewrite of the phone-side software that drives the [Telephone Booth][cafka]
art installation. It owns the hardware: rotary-dial pulses, hook-switch edges,
USB audio recording (Focusrite or any UAC2 device), and FLAC playback. It talks
to the [operator backend](https://github.com/djensenius/Telephone-Booth-Operator)
over a small typed REST + WebSocket API.

The original 2016 Node.js implementation lives on the `legacy-node` tag.

[cafka]: http://www.cafka.org/cafka16/08-david-jensenius-kitchener-telephone-booth

## Highlights

- **Hexagonal architecture** — a pure, `no_std`-friendly state-machine core
  (`booth-core`) sits behind a trait-based HAL (`booth-hal`). The Pi adapter
  (`booth-pi`) plugs hardware into the same shape that a future ESP32 or Pico
  adapter would.
- **Hard real-time-ish flow on the dial:** rotary pulses are debounced and
  decoded into digits, the state machine handles every legal transition (and
  rejects the rest), and effects are dispatched through the HAL.
- **Built-in debug surface** (`booth-debug`) — embedded HTTPS server that
  exposes pin matrix, state history, audio meters, and raw-event telemetry
  over WebSocket. Reachable over Tailscale (real Let's Encrypt cert) or LAN
  (self-signed + fingerprint pin).
- **First-class observability** — every booth ships a `booth-metrics`
  registry on `/metrics` (loopback only, scraped by a vmagent sidecar
  that remote_writes to VictoriaMetrics) plus a thorough event log that
  the operator persists in Postgres. See
  [`docs/observability.md`](./docs/observability.md).
- **Everything is tested** — `proptest` over state-machine transitions, snapshot
  tests via `insta`, integration tests against `booth-mock`, and a CI matrix
  that cross-compiles for Pi 3/4/5 (`armv7-unknown-linux-gnueabihf` and
  `aarch64-unknown-linux-gnu`).

## Quickstart

```bash
mise install              # Rust 1.95.0 + just + cargo-nextest, etc.
just setup
just dev                  # runs against the mock HAL on your laptop
just tui                  # interactive simulator TUI (mock GPIO + mock I/O)
just check                # fmt + clippy + tests + docs lint (what CI runs)
```

The interactive simulator (`just tui` or `cargo run -p booth-bin -- run --simulator`)
mimics the rotary phone from your keyboard so you can exercise the full
booth pipeline — state machine, audio, and operator HTTP client — without
any hardware attached. See [`docs/simulator.md`](./docs/simulator.md) for
the full key bindings and modes.

To run on a real Pi with a Focusrite (or any USB-Audio-Class-2 device):

```bash
just cross-build aarch64-unknown-linux-gnu
just deb
scp target/aarch64-unknown-linux-gnu/debian/*.deb pi@booth:
ssh pi@booth "sudo apt install ./telephone-booth_*_arm64.deb"
```

Full setup (wiring, config keys, Tailscale, debug-surface auth, packaging,
porting to ESP32/Pico, runbook, ADRs) is in [`docs/`](./docs/README.md).

## Repository layout

```text
crates/
├── booth-core/   pure state machine, no IO (no_std-friendly)
├── booth-hal/    trait-based HAL: GpioPort, AudioSink/Source, OperatorClient, …
├── booth-mock/   host-runnable mock adapters used by integration tests
├── booth-debug/  axum debug HTTP/WS surface + minimal embedded htmx UI
├── booth-pi/     Pi adapter: rppal GPIO, cpal/ALSA audio, reqwest + tokio
└── booth-bin/    binary that wires it all together and runs the loop
docs/             user, operator, and porting documentation
assets/
└── debug-ui/     embedded standalone debug UI (htmx)
.github/workflows/
├── ci.yml        fmt + clippy + tests + cross-build matrix + docs lint
├── audit.yml     cargo-deny + cargo-audit on a schedule
└── publish.yml   workflow_dispatch: builds release artefacts (.deb + tarballs)
```

## License

[BSD-3-Clause](./LICENSE).
