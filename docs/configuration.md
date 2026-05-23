# Configuration

The client merges config from three sources, in this order (later wins):

1. **Defaults** baked into `booth-pi::PiConfig::default()`.
2. **`/etc/phone-booth/config.toml`** (installed by the `.deb`).
3. **Environment variables** prefixed with `PHONE_BOOTH_`
   (e.g. `PHONE_BOOTH_OPERATOR_TOKEN`).

Run `telephone-booth --print-config` at any time to dump the effective
merged config with secrets redacted to the last 4 characters.

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
recordings_dir     = "/var/lib/telephone-booth/recordings"
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
token_file           = "/etc/phone-booth/debug-token"
cert_file            = "/etc/phone-booth/debug-cert.pem"
key_file             = "/etc/phone-booth/debug-key.pem"
fingerprint_file     = "/etc/phone-booth/debug-cert.fingerprint"

[telemetry]
journal_level = "info"        # tracing filter
```

## Environment variables

Every key in the file can be overridden via env. Nested keys use `__` as a
separator (so they survive shells that disallow dots in names):

| File key                 | Env override                          |
| ------------------------ | ------------------------------------- |
| `gpio.hook_bcm`          | `PHONE_BOOTH_GPIO__HOOK_BCM`          |
| `audio.device_substring` | `PHONE_BOOTH_AUDIO__DEVICE_SUBSTRING` |
| `operator.token`         | `PHONE_BOOTH_OPERATOR__TOKEN`         |
| `debug.allow_controls`   | `PHONE_BOOTH_DEBUG__ALLOW_CONTROLS`   |

## Secret precedence

Two secrets are never written to the journal:

- `operator.token` — phone-client API token.
- `debug.token` — Bearer for the local debug surface.

Both can be supplied via either the file or the env. On first run, if
`debug.token_file` does not exist, the runtime generates a fresh 256-bit
token, writes it `0600` to the file, and prints it once to the journal so
you can copy it.
