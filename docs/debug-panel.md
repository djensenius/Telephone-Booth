# Debug panel

Every Rust booth ships an **always-on local diagnostics server** embedded in
the binary. It's how you confirm a freshly-installed booth is actually
seeing rotary pulses, picking up the right audio device, and reaching the
operator.

This panel is **completely independent** of the operator backend, so it
keeps working when the operator is offline.

## Transports

The panel is reachable two ways at once; either listener can be disabled
via `debug.tailscale_enabled` / `debug.lan_enabled`.

| Transport             | URL                                       | TLS                                       |
| --------------------- | ----------------------------------------- | ----------------------------------------- |
| **Tailscale serve** (default) | `https://telephone-booth.<your-tailnet>.ts.net` | Real Let's Encrypt cert issued by Tailscale |
| **LAN fallback**      | `https://<pi-ip>:8443`                    | Self-signed (rcgen) cert, fingerprint-pinned |

For Tailscale, see [`tailscale.md`](tailscale.md); for the LAN fallback
with fingerprint pinning, see [`lan-fallback.md`](lan-fallback.md).

## Authentication

Every request — HTTP **and** WebSocket — must present the debug token.

```http
Authorization: Bearer <debug token>
```

For WebSocket, the operator UI passes the token in the
`Sec-WebSocket-Protocol: bearer.<token>` subprotocol header so it isn't
logged in URLs.

The packaged service reads the token from `BOOTH_DEBUG_TOKEN` in
`/etc/phone-booth/env`. To rotate it, edit that value, restart
`telephone-booth.service`, and update the operator UI.

## Endpoints

| Method   | Path                                | Purpose                                              |
| -------- | ----------------------------------- | ---------------------------------------------------- |
| `GET`    | `/`                                 | Standalone htmx + vanilla-JS panel UI                 |
| `GET`    | `/debug/state`                      | Current state machine state + recent transitions      |
| `GET`    | `/debug/gpio`                       | Configured pins + last N edge events                  |
| `GET`    | `/debug/audio`                      | Selected device, sample rate, levels, recent files    |
| `GET`    | `/debug/operator`                   | Operator reachability, last upload, WS state          |
| `GET`    | `/debug/logs?level=info&tail=200`   | Tail of tracing logs (JSON lines)                     |
| `GET`    | `/debug/config`                     | Effective config with secrets redacted                |
| `WS`     | `/debug/stream?since=<event_id>`    | Raw event firehose; ring-buffer replay on reconnect   |
| `POST`   | `/debug/simulate/gpio`              | Inject simulated GPIO edge (controls)                 |
| `POST`   | `/debug/simulate/digit`             | Inject a complete dial (controls)                     |
| `POST`   | `/debug/hangup`                     | Force state machine to `Idle` (controls)              |
| `POST`   | `/debug/replay-last-recording`      | Play the most recent recording out the sink (controls) |

In addition to the historical `/debug/*` aliases, the runtime today
exposes the canonical `v1` routes:

| Method   | Path                | Purpose                                                                 |
| -------- | ------------------- | ----------------------------------------------------------------------- |
| `GET`    | `/v1/state`         | Latest [`StatusSnapshot`](../crates/booth-debug/src/lib.rs).            |
| `GET`    | `/v1/system`        | Latest [`SystemSnapshot`](observability.md#systemsnapshot-fields).      |
| `GET`    | `/v1/events?since=` | Telemetry ring-buffer replay.                                           |
| `GET`    | `/v1/ws/telemetry`  | Live telemetry firehose.                                                |
| `GET`    | `/metrics`          | Prometheus text exposition. **Loopback only**, no bearer auth required. |

The `/metrics` route is only mounted on the loopback listener (so
`tailscale serve` exposes it under the existing ACL) and intentionally
skips the bearer-auth middleware so the
`telephone-booth-vmagent.service` sidecar can scrape it without
credentials. See [observability.md](observability.md) and
[ADR 0006](adr/0006-observability-stack.md).

**Routes marked _(controls)_** are gated by `debug.allow_controls = true`
in the config file. The flag is intentionally _not_ toggle-able at runtime;
flipping it requires editing config and restarting the service.

## Telemetry event schema

Every event on `/debug/stream` is one JSON object per line:

```json
{ "id": 1234, "ts": "2026-04-01T12:34:56.789Z", "kind": "gpio.edge",
  "role": "rotary_pulse", "level": "low" }
{ "id": 1235, "ts": "…", "kind": "digit.dialed", "digit": 5 }
{ "id": 1236, "ts": "…", "kind": "state.transition",
  "from": "DialTone", "to": "Beep" }
{ "id": 1237, "ts": "…", "kind": "audio.level",
  "channel": "input", "peak_dbfs": -8.3, "rms_dbfs": -22.1 }
```

`id` is a monotonically increasing event counter; pass `?since=<id>` to
catch up after a reconnect.

## Standalone UI

`GET /` (after Bearer auth via a one-time token in the URL fragment or a
cookie set by the operator UI) returns a small embedded htmx page that
shows the same data the operator UI's Debug tab does. It exists so you can
debug a booth from a phone browser on the local network when the operator
backend is down.

See [`runbook.md`](runbook.md) for token-rotation and cert-regeneration
procedures.
