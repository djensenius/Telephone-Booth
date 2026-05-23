# ESP32 porting skeleton

> **Status:** future / not yet implemented. This document is the recipe.

The booth runs comfortably on a Pi today, but the architecture is
deliberately reachable on an ESP32-S3 with PSRAM. This page sketches what
a `booth-esp32` adapter crate would look like.

## What an ESP32 booth gives up

- The embedded debug HTTP server (`booth-debug`) is too heavy; consider a
  much smaller `/debug/stream` over MQTT or USB-serial instead.
- FLAC encoding on-device is borderline; you may want to stream PCM to
  the operator and encode there.

## Crate skeleton

```
crates/booth-esp32/
├── Cargo.toml          esp-hal, embassy, embedded-svc, esp-wifi
├── src/
│   ├── lib.rs          re-exports + the runtime entrypoint
│   ├── gpio.rs         impl GpioPort using esp-hal::gpio
│   ├── audio.rs        impl AudioSink / AudioSource via I2S
│   ├── operator.rs     impl OperatorClient via embedded-svc HTTP
│   └── clock.rs        impl Clock via embassy_time
└── examples/
    └── booth-s3.rs     `main` for a specific board
```

## Key crates

| Concern | Recommended crate          |
| ------- | -------------------------- |
| HAL     | `esp-hal`                   |
| Async   | `embassy-executor`         |
| Net     | `embassy-net` + `esp-wifi` |
| TLS     | `embedded-tls`              |
| HTTP    | `embedded-svc` HTTP client  |
| Audio   | `esp-hal-embassy` I2S + `claxon` (FLAC decode) |

## Memory budget

Target: **< 200 KB SRAM**, **< 8 MB PSRAM** for recordings/queue.

## CI

Add `.github/workflows/ci.yml` matrix entry:

```yaml
- target: xtensa-esp32s3-none-elf
  toolchain: esp
  runner: ubuntu-latest
```

…with the `esp-rs/xtensa-toolchain` action.

## Testing

The `booth-core` unit tests already run on `no_std + alloc`; add a
`booth-esp32` integration test using the `embedded-test` framework if you
want hardware-in-the-loop coverage.
