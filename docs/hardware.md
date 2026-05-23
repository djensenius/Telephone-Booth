# Hardware

The Rust client targets a Raspberry Pi (any model with the 40-pin header) with
a **USB-Audio-Class 2.0** audio interface plugged in — a Focusrite Scarlett
Solo / 2i2 is the reference device, but any UAC2 interface should work.

## Rotary phone wiring

The booth uses three GPIO inputs against ground, debounced in software:

| Function       | Default BCM pin | Physical pin | Wire color (typical) |
| -------------- | --------------- | ------------ | --------------------- |
| Hook switch    | BCM 17          | 11           | green                 |
| Rotary pulse   | BCM 27          | 13           | yellow                |
| Rotary "dialing" gate | BCM 22   | 15           | blue                  |

Ground is physical pin 9 (any GND pin on the header works).

All three inputs are configured with the Pi's internal pull-up resistor by default
(`rppal` `PullUp`, overridable to `PullDown`) and read **active-low** when wired
as contacts to ground (closed = 0). The state machine treats:

- Hook switch closed → `HookOn`; open → `HookOff`.
- Rotary "dialing" gate **closed** while the user spins the dial; on the
  trailing edge the runtime emits `DigitClosed(N)` for the count of pulses
  collected while the gate was closed.
- Each pulse (closing of the pulse contact while the gate is closed) is
  emitted as `RotaryPulse` after a 5 ms debounce.

If your phone wiring is reversed, set `gpio.invert.<role> = true` in the
config file — see [`configuration.md`](configuration.md). The ignored
`booth-pi` loopback test documents a hardware smoke test using an output pin
wired to one of these inputs.

### Pin mapping defaults

The defaults above (BCM 17/27/22 → physical 11/13/15) match the wiring of
the original Node.js installation, so existing booths can be re-flashed
without re-soldering. Every pin is overridable:

```toml
# /etc/phone-booth/config.toml
[gpio]
hook_bcm        = 17
rotary_pulse_bcm = 27
rotary_gate_bcm  = 22
pull             = "up"      # or "down"
debounce_ms      = 5
```

## USB audio device

`cpal` enumerates UAC2 devices automatically. To survive USB reordering
across reboots, the config selects by **device-name substring** (case-
insensitive):

```toml
[audio]
device_substring = "Focusrite"
sample_rate_hz   = 48000
channels         = 1
max_recording_secs = 60
```

If no matching device is found at startup, the runtime falls back to the
system default input/output and logs a warning. Recording then fails fast
with a clear error rather than silently writing zeros.

### Microphone level

The Scarlett's analog "INST" / "MIC" gain wheel sets the input level. Aim
for the level meter in the debug UI to peak around -6 dBFS while someone
speaks at booth distance. The runtime publishes peak/RMS samples to the
telemetry bus roughly every 50 ms.

### Recording format

All recordings are **FLAC** (lossless, mono, 48 kHz) — see
[ADR 0003](adr/0003-flac-as-recording-format.md). Files are stored at
`/var/lib/telephone-booth/recordings/<sha256>.flac` and uploaded to Azure
Blob Storage via a presigned SAS URL.

## Power & boot

The Pi should boot off an SD card (or, preferably, an SSD via USB3) running
Raspberry Pi OS 64-bit. The systemd unit installed by the `.deb` package
waits for `network-online.target` so the client never tries to contact the
operator before networking is up. See [`packaging.md`](packaging.md).
