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
