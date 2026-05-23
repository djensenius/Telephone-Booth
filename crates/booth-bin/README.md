# booth-bin

`telephone-booth` is the phone-side runtime binary. It loads the Pi config, wires the `booth-pi` or `booth-mock` HAL adapters into `booth-core`, publishes telemetry, and starts the embedded debug surface.

## CLI

```sh
telephone-booth run [--config <path>] [--mock]
telephone-booth print-config [--config <path>]
telephone-booth check [--config <path>]
telephone-booth simulate <pulses>
```

- `run` starts the runtime. Config defaults to `/etc/phone-booth/config.toml`, falling back to `./config.toml` when present. `--mock` uses `booth-mock`; it requires the `mock` Cargo feature (enabled by default for host development).
- `print-config` prints the effective config as TOML with tokens redacted.
- `check` validates config and probes the Pi adapters. It exits nonzero on failure and is suitable for systemd `ExecStartPre`.
- `simulate` runs the pure state machine with `HookOff`, N rotary pulses, then `Tick` and prints states/effects.

## Runtime tasks

When `observability.enabled = true` (default), `run` spawns three
additional background tasks alongside the main GPIO/audio/effect tasks:

- **System sampler** (from `booth-metrics`) — periodically samples
  CPU/temp/mem/disk/net via `sysinfo`, updates the Prometheus
  registry, and publishes `TelemetryEvent::SystemSample` onto the
  bus.
- **Event forwarder** — buffers every telemetry event into batches and
  `POST /v1/events` on the operator, with idempotent `event_id`s and
  a drop-oldest bounded queue.
- **System pusher** — `PUT /v1/system` to the operator on every
  sample so the Live System panel stays current.

The captured `booth_metrics::MetricsHandle` is also passed into
`booth_debug::serve_with_handles` so the loopback `/metrics` endpoint
can render the current registry. See
[`docs/observability.md`](../../docs/observability.md) for the full
data flow and metric catalog.
