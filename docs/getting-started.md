# Getting started

You'll need a working Rust toolchain (we pin **1.95.0**) and a Linux or macOS
host. On the Pi, the binary runs as a `systemd` service installed from a
`.deb`; for development, you can run the host binary against the mock HAL.

## 1. Install the toolchain

We use [`mise`](https://mise.jdx.dev) to pin every tool the repo needs.

```sh
brew install mise            # or: curl https://mise.run | sh
git clone https://github.com/djensenius/Telephone-Booth.git
cd Telephone-Booth
git switch rust-client
mise install                 # installs Rust 1.95.0, just, cargo-deb, cross, …
```

If you'd rather not use `mise`, install Rust 1.95.0 directly via
[`rustup`](https://rustup.rs) — `rust-toolchain.toml` will pick up the pinned
version automatically.

## 2. Sanity check

```sh
just check                   # fmt + clippy + tests
```

The default `cargo test --workspace` run uses the in-memory mock HAL, so it
needs no hardware.

## 3. Run with the mock HAL

```sh
cargo run -p booth-bin -- --print-config
```

`--print-config` echoes the effective configuration with secrets redacted.
Drop the flag to start the runtime; the debug HTTP server comes up on
`127.0.0.1:8080` (Tailscale-serve target) and `127.0.0.1:8443` (LAN
fallback, disabled by default). The debug token is generated on first run
and printed once to stdout.

## 4. Next steps

### Understand the architecture

- **[Architecture overview](architecture.md)** — Learn the hexagonal design, state machine, and event/effect model
- **[Simulator TUI](simulator.md)** — Test the full state machine interactively without hardware

### Deploy to production

- **[Raspberry Pi setup](raspberry-pi-setup.md)** — Flash, configure, and wire a physical booth
- **[Hardware wiring](hardware.md)** — GPIO pinout and rotary phone connections
- **[Tailscale setup](tailscale.md)** — Remote access with automatic HTTPS certificates

### Contribute

- **[Contributing guide](contributing.md)** — Coding conventions, PR process, ADRs
