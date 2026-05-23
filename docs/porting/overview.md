# Porting overview

The Rust client is built so that **the only thing that changes when you
move to a new platform is which `booth-<adapter>` crate implements the
HAL traits.** `booth-core` (the state machine) and `booth-hal` (the trait
definitions) don't change.

## What you implement

| Trait              | What it does                                                         | Required for ESP32 / Pico? |
| ------------------ | -------------------------------------------------------------------- | -------------------------- |
| `GpioPort`         | Read hook + pulse + gate, emit debounced edge events                  | yes                        |
| `AudioSink`        | Play a `Builtin(Tone)` or `RemoteUrl(audio/flac)` clip                | yes (decode-capable)       |
| `AudioSource`      | Capture mono FLAC into `Storage`                                      | yes if you want recordings  |
| `OperatorClient`   | Talk to the operator backend (HTTPS + WS)                             | depends on your transport   |
| `Clock`            | Monotonic time source                                                 | yes                        |
| `Storage`          | Persist recordings until upload                                       | yes if recording            |

The traits live in `crates/booth-hal/src/lib.rs`. They use `alloc` (not
`std`) and are object-safe; on `no_std` targets you'll want
`alloc::sync::Arc<dyn …>` style boxing.

## What you keep

- `crates/booth-core` — the state machine. Builds on `no_std + alloc`.
- `crates/booth-hal` — trait definitions.
- Anything in `crates/booth-pi/src/config.rs` that's purely data.

## What's likely to need an adaptation

- **Async runtime.** Today the Pi adapter uses `tokio`. ESP32 uses
  `embassy`; Pico has `embassy` too. Both wrap their own executor over
  the HAL futures. The state machine itself is sync, so this only
  affects the runtime glue.
- **HTTP/WS client.** `reqwest` is std-only. For ESP32 you'd swap in
  `embedded-svc` HTTP + `embedded-tls`. On a Pico without Wi-Fi you'd
  return `Err(OperatorClientError::NotSupported)` for everything.
- **Audio.** FLAC encoding requires a fair amount of RAM. On a Pico, you
  may want to record straight to PCM and let a companion device transcode.

## CI matrix

When you add a port, also add a row to `.github/workflows/ci.yml`'s
`cross-build` job so future changes can't accidentally break it.

See:

- [ESP32 skeleton](esp32.md)
- [Pico skeleton](pico.md)
