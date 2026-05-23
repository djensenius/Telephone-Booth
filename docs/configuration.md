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
debounce_ms      = 5
invert.hook      = false
invert.rotary_pulse = false
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
lan_enabled          = true
lan_bind             = "0.0.0.0:8443"
loopback_bind        = "127.0.0.1:8080"
allow_controls       = false
ring_buffer_capacity = 4096
loopback_skip_auth   = false
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
```

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

Other observability settings (`sample_interval_ms`, `batch_max`,
`flush_interval_ms`, `buffer_max`) are config-file only and have no
env override.

## Secret precedence

Two secrets are never written to the journal:

- `operator.token` — phone-client API token.
- debug bearer token — supplied via `BOOTH_DEBUG_TOKEN` or `BOOTH_DEBUG_TOKEN_FILE`.

For secrets, the direct env var wins over its `*_FILE` partner. File values are
trimmed for trailing newlines so they work with systemd credentials and
Kubernetes-style secret mounts.
