# Raspberry Pi Pico porting skeleton

> **Status:** future / not yet implemented.

A Pico W (RP2040 + CYW43439 Wi-Fi) makes a fun minimal booth. This page
sketches the adapter.

## Constraints

- **264 KB SRAM total.** FLAC encoding is realistic only with a small
  ring buffer + streaming upload; expect ~5 s max recording.
- No filesystem; uploads must go straight to the operator (or to an SD
  card via SPI).

## Crate skeleton

```text
crates/booth-pico/
├── Cargo.toml          embassy-rp, embassy-net, cyw43, embedded-svc
├── src/
│   ├── lib.rs
│   ├── gpio.rs         impl GpioPort with embassy_rp::gpio
│   ├── audio.rs        impl AudioSink via PWM/I2S; AudioSource via ADC + DMA
│   ├── operator.rs     impl OperatorClient via embedded-svc + embedded-tls
│   └── clock.rs        impl Clock via embassy_time
└── examples/
    └── booth-pico-w.rs
```

## Wiring

The Pico's GPIO maps 1:1 to the same logical roles as the Pi: pick three
GPIOs, wire them to hook, pulse, gate with the internal pull-ups, and set
them in `config.toml` equivalent.

## CI

```yaml
- target: thumbv6m-none-eabi
  toolchain: 1.95.0
  runner: ubuntu-latest
```

Use `flip-link` for stack overflow protection and `defmt` + `probe-rs`
for debugging.
