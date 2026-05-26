# booth-debug

Embedded debug HTTP + WebSocket surface for the Telephone Booth Rust client.

## Endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/healthz` | Health and crate version. |
| `GET` | `/v1/state` | Current operator-compatible booth status snapshot. |
| `GET` | `/v1/events?since=<seq>` | Retained telemetry records with ids greater than `since`. |
| `GET` | `/v1/gpio` | Latest GPIO pin levels and edge timestamps from telemetry. |
| `GET` | `/v1/audio` | Latest input/output audio meter values and device info. |
| `GET` | `/v1/system` | Latest [`SystemSnapshot`](../../docs/observability.md#systemsnapshot-fields) (CPU/temp/mem/disk/net/uptime). |
| `GET` | `/v1/logs?level=info&limit=200` | Recent tracing log lines from the in-process ring buffer. |
| `GET` | `/v1/config` | Effective config projection with tokens redacted to the last 4 chars. Includes `debug.runtimeMode` so the web UI can tell whether controls are live. |
| `GET` | `/v1/cert/fingerprint` | Loopback-only SHA-256 fingerprint for LAN cert pinning. |
| `POST` | `/v1/simulate/event` | Inject a serialized `booth_core::Event`. Gated — see *Simulation controls* below. |
| `POST` | `/v1/simulate/pulse` | Inject N rotary pulses followed by `Tick`. Gated — see *Simulation controls* below. |
| `WS` | `/v1/ws/telemetry` | Live `TelemetryRecord` JSON frames; optional first message `{\"replay_from\": seq}`. |
| `GET` | `/metrics` | **Loopback only.** Prometheus text exposition. Skips bearer auth — Tailscale ACLs gate the loopback front door. |

All HTTP and WebSocket requests require `Authorization: Bearer <debug-token>` when `DebugConfig::token` is set, **except** `/metrics`, which is intentionally unauthenticated and mounted only on the loopback listener so vmagent can scrape it. WebSocket clients may also pass `Sec-WebSocket-Protocol: bearer.<token>`.

The metrics renderer is supplied by the runtime as an
`Option<MetricsRender>` argument to [`serve_with_handles`]; when `None`,
the `/metrics` route is not mounted at all (404).

## Simulation controls

Simulation endpoints are disabled by default. Enable them only for trusted debugging sessions by setting:

```toml
[debug]
allow_controls = true
```

The flag is read at startup and is intentionally not toggleable over the debug API.

**Hardware-mode guard.** Even when `allow_controls = true`, both
`/v1/simulate/event` and `/v1/simulate/pulse` return `403 Forbidden` with
`{"error":"controls_denied","reason":"headless_real_hardware",...}` when
the booth is composed with `RuntimeMode::Real` (real GPIO, audio, and
operator HTTP). Synthetic events are accepted only under
`RuntimeMode::Mock` or `RuntimeMode::Simulator`, so live hardware never
competes with injected events. The embedded web UI reads `/v1/config`,
shows a "headless / real-hardware mode" banner, and disables the hook
and dial controls when the runtime is `real`.
