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
| `GET` | `/v1/logs?level=info&limit=200` | Recent tracing log lines from the in-process ring buffer. |
| `GET` | `/v1/config` | Effective config projection with tokens redacted to the last 4 chars. |
| `GET` | `/v1/cert/fingerprint` | Loopback-only SHA-256 fingerprint for LAN cert pinning. |
| `POST` | `/v1/simulate/event` | Inject a serialized `booth_core::Event`. |
| `POST` | `/v1/simulate/pulse` | Inject N rotary pulses followed by `Tick`. |
| `WS` | `/v1/ws/telemetry` | Live `TelemetryRecord` JSON frames; optional first message `{\"replay_from\": seq}`. |

All HTTP and WebSocket requests require `Authorization: Bearer <debug-token>` when `DebugConfig::token` is set. WebSocket clients may also pass `Sec-WebSocket-Protocol: bearer.<token>`.

## Simulation controls

Simulation endpoints are disabled by default. Enable them only for trusted debugging sessions by setting:

```toml
[debug]
allow_controls = true
```

The flag is read at startup and is intentionally not toggleable over the debug API.
