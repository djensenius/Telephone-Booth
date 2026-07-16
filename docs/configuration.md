# Configuration

The client merges config from three sources, in this order (later wins):

1. **Defaults** baked into `booth-pi::PiConfig::default()` plus runtime/debug defaults.
2. **`/etc/phone-booth/config.toml`** (installed by the `.deb`), falling back to
   `./config.toml` for host development when the production file is absent.
3. **Environment variables** prefixed with `BOOTH_`
   (for example `BOOTH_OPERATOR_TOKEN`).

Run `telephone-booth print-config` at any time to dump the effective merged
config as TOML with secrets redacted to the last 4 characters.

## Full example

```toml
# /etc/phone-booth/config.toml

[gpio]
hook_bcm         = 17        # physical pin 11
rotary_pulse_bcm = 27        # physical pin 13
rotary_gate_bcm  = 22        # physical pin 15
pull             = "up"      # "up" | "down"
debounce_ms      = 25
invert.hook      = false
invert.rotary_pulse = true
invert.rotary_gate  = false

[audio]
device_substring   = "Focusrite"
sample_rate_hz     = 48000
channels           = 1
max_recording_secs = 60
recordings_dir     = "/var/lib/phone-booth/recordings"
beep_volume        = 0.8
dialtone_volume    = 0.6

[operator]
base_url    = "https://operator.example.com"
token       = "tbo_REPLACE_WITH_TOKEN"
status_topic = "booth-1"
http_timeout_secs = 10
ws_reconnect_initial_ms = 500
ws_reconnect_max_ms     = 30000

[debug]
tailscale_enabled    = true
lan_enabled          = false
lan_bind             = "127.0.0.1:8443"
loopback_bind        = "127.0.0.1:8080"
allow_controls       = false
ring_buffer_capacity = 4096
loopback_skip_auth   = false
allow_tokenless      = false
# Set the bearer token with BOOTH_DEBUG_TOKEN or BOOTH_DEBUG_TOKEN_FILE.

[telemetry]
journal_level = "info"        # tracing filter

[observability]
enabled            = true     # master switch for metrics + operator forwarding
booth_id           = "booth-01"
sample_interval_ms = 5000     # how often booth-metrics samples sysinfo

[observability.operator_forward]
enabled           = true
batch_max         = 200       # events per POST /v1/events
flush_interval_ms = 2000
buffer_max        = 4096      # hard cap; drop-oldest on overflow

[runtime]
# Autostart mode. Both default to false. CLI flags (--mock, --simulator) can
# force a mode on at launch but cannot force it off.
mock      = false
simulator = false
```

### Runtime startup mode

`[runtime]` lets the systemd unit autostart the booth in `--mock` or
`--simulator` mode without editing `ExecStart`. Both flags default to
`false`, in which case the binary uses the real Pi adapters and runs
headless — the historical behaviour.

| Key                  | Effect                                                                                       |
| -------------------- | -------------------------------------------------------------------------------------------- |
| `runtime.mock`       | Use the in-memory `booth-mock` HAL adapters instead of Pi hardware. Same as `--mock`.        |
| `runtime.simulator`  | Launch the interactive `ratatui` TUI on startup. Same as `--simulator`.                      |

Both require the binary to be built with the matching Cargo feature
(`mock`, `simulator`). The published `.deb` includes both; if you build
a custom binary with `--no-default-features --features pi,systemd`
(only) then setting either flag to `true` will fail `validate_config`
at startup with an explicit error.

`simulator = true` requires a TTY for the TUI. The default
`telephone-booth.service` unit does **not** allocate one, so
autostarting in simulator mode needs a systemd override:

```ini
# /etc/systemd/system/telephone-booth.service.d/simulator.conf
[Service]
StandardInput=tty
StandardOutput=tty
TTYPath=/dev/tty1
```

`mock = true` has no such requirement and works under the stock unit —
useful for bringing up a Pi without the rotary phone wired in.

Whichever mode the booth resolves to is also surfaced to the operator: the
runtime stamps every `PUT /v1/status` and `PUT /v1/system` payload with a
`runtimeMode` field (`real`, `mock`, or `simulator`) and exports it as a
bounded `booth_info{mode=…}` gauge for Grafana / VictoriaMetrics. The
operator UI uses the field to render a `MOCK` or `SIM` badge so dashboard
viewers can tell synthetic booths apart at a glance. Simulator mode wins
over mock when both are set, because "TUI is driving input" is a more
user-visible fact than "mock adapters underneath" — the simulator can be
paired with the real `booth-pi` audio + operator adapters and the badge
should still say `SIM`.

### Observability

When `observability.enabled = true` (the default) the runtime installs
the Prometheus registry, spawns the system sampler, and — if
`operator_forward.enabled = true` — forwards every telemetry event to
the operator API as a `BoothEvent` row and pushes the latest
`SystemSnapshot` every sample.

Remote write to VictoriaMetrics is **not** controlled here — that's
vmagent's job. See [`observability.md`](observability.md#packaging)
for the vmagent unit and the `BOOTH_VM_REMOTE_WRITE_URL` env var that
lives in `/etc/phone-booth/vmagent.env`.

### Upload caps

Before contacting the operator, the phone refuses recordings that exceed the
operator's hard limits: `booth_pi::MAX_UPLOAD_BYTES` is `26_214_400` (25 MiB)
and `booth_pi::MAX_UPLOAD_DURATION_MS` is `300_000` (5 minutes). Rejected
recordings stay on disk and in the pending-upload spool for operator-visible
triage instead of being discarded.

## Environment variables

The runtime currently supports explicit overrides for deployment-sensitive
settings:

| File key / setting                              | Env override                                                                      |
| ----------------------------------------------- | --------------------------------------------------------------------------------- |
| `operator.base_url`                             | `BOOTH_OPERATOR_BASE_URL`                                                         |
| `operator.token`                                | `BOOTH_OPERATOR_TOKEN` or `BOOTH_OPERATOR_TOKEN_FILE`                             |
| debug bearer token                              | `BOOTH_DEBUG_TOKEN` or `BOOTH_DEBUG_TOKEN_FILE`                                   |
| `audio.device_substring`                        | `BOOTH_AUDIO_DEVICE`                                                              |
| `gpio.hook`                                     | `BOOTH_GPIO_HOOK` or `BOOTH_GPIO_HOOK_BCM`                                        |
| `gpio.rotary_pulse`                             | `BOOTH_GPIO_ROTARY_PULSE` or `BOOTH_GPIO_ROTARY_PULSE_BCM`                        |
| `gpio.rotary_read`                              | `BOOTH_GPIO_ROTARY_READ`, `BOOTH_GPIO_ROTARY_READ_BCM`, or `BOOTH_GPIO_ROTARY_GATE_BCM` |
| `gpio.debounce_ms`                              | `BOOTH_GPIO_DEBOUNCE_MS`                                                          |
| `gpio.pull`                                     | `BOOTH_GPIO_PULL` (`up` or `down`)                                                |
| `gpio.invert.*`                                 | `BOOTH_GPIO_INVERT_HOOK`, `BOOTH_GPIO_INVERT_ROTARY_PULSE`, `BOOTH_GPIO_INVERT_ROTARY_READ` |
| `observability.enabled`                         | `BOOTH_OBSERVABILITY_ENABLED`                                                     |
| `observability.booth_id`                        | `BOOTH_OBSERVABILITY_BOOTH_ID`                                                    |
| `observability.operator_forward.enabled`        | `BOOTH_OBSERVABILITY_FORWARD_ENABLED`                                             |
| `runtime.mock`                                  | `BOOTH_RUNTIME_MOCK`                                                              |
| `runtime.simulator`                             | `BOOTH_RUNTIME_SIMULATOR`                                                         |

Other observability settings (`sample_interval_ms`, `batch_max`,
`flush_interval_ms`, `buffer_max`) are config-file only and have no
env override.

## Secret precedence

Two secrets are never written to the journal:

- `operator.token` — phone-client API token.
- debug bearer token — supplied via `BOOTH_DEBUG_TOKEN` or `BOOTH_DEBUG_TOKEN_FILE`.

The debug surface **fails closed**: if no bearer token is configured and
`allow_tokenless` is not explicitly set to `true`, the debug listeners refuse
to start. This prevents accidental exposure due to a missing env file or
provisioning error. Set `allow_tokenless = true` in the `[debug]` section only
for local development where network exposure is not a concern.

For secrets, the direct env var wins over its `*_FILE` partner. File values are
trimmed for trailing newlines so they work with systemd credentials and
Kubernetes-style secret mounts.
