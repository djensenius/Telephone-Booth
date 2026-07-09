# Telephone-Booth client — documentation

The phone-side Rust client lives in this repo on the `rust-client` branch.

This index is the source of truth for the docs tree. `just docs-index`
rebuilds it from the filesystem; CI fails if it drifts.

## Start here

- [Project overview](overview.md) — what this is, who it's for, architecture at a glance

## About the work

- [Artist statement & bio](artist-statement.md) — the installation's concept, site-specific context, and artist bio

## Setup & Development

- [Raspberry Pi setup](raspberry-pi-setup.md) — from-scratch Pi provisioning (OS, audio, Tailscale, deploy, wiring)
- [Getting started](getting-started.md) — clone → mise install → `just dev`
- [Simulator TUI](simulator.md) — interactive `--simulator` flag for local testing without a phone
- [Hardware](hardware.md) — wiring the rotary phone + GPIO pinout
- [Payphone reference](payphone-reference.md) — physical 3-slot payphone subsystem breakdown + service-manual links
- [Configuration](configuration.md) — every config key, its default, env-var equivalent

## Running it

- [Packaging](packaging.md) — building the `.deb`, installing via systemd
- [Operator API](operator-api.md) — registering with the operator backend, token rotation
- [Tailscale](tailscale.md) — exposing the debug surface over your tailnet
- [LAN fallback](lan-fallback.md) — self-signed certs + fingerprint pinning

## Inside the box

- [Architecture](architecture.md) — hexagonal layout, state machine, event/effect catalog
- [Debug panel](debug-panel.md) — endpoints, auth, telemetry schema, htmx UI tour
- [Observability](observability.md) — events, host vitals, `/metrics`, vmagent + Grafana

## Future portability

- [Porting overview](porting/overview.md) — what the HAL boundary means
- [ESP32 skeleton](porting/esp32.md)
- [Pico skeleton](porting/pico.md)

## When things go wrong

- [Troubleshooting](troubleshooting.md) — symptoms ↔ causes ↔ fixes
- [Runbook](runbook.md) — day-2 ops

## For contributors

- [Contributing](contributing.md)

## ADRs

- [0001 — Hexagonal architecture](adr/0001-hexagonal-architecture.md)
- [0002 — State machine as a pure core](adr/0002-state-machine-as-pure-core.md)
- [0003 — FLAC as recording format](adr/0003-flac-as-recording-format.md)
- [0004 — Tailscale serve for debug TLS](adr/0004-tailscale-serve-for-debug-tls.md)
- [0005 — Rust 1.95.0](adr/0005-rust-1.95.0.md)
- [0006 — Observability stack](adr/0006-observability-stack.md)
- [0007 — APT distribution](adr/0007-apt-distribution.md)
- [0008 — Automated releases](adr/0008-automated-releases.md)
