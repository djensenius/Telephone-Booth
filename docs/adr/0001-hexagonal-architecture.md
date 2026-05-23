# ADR 0001 — Hexagonal architecture

**Status:** accepted.

## Context

The original Node.js code mixed GPIO reads, file IO, HTTP calls, and
state transitions in a single ~300 LOC file. That made it impossible to
test without a Pi, hard to reason about edge cases, and tightly coupled
to the specific platform.

We also want to keep the door open for non-Pi targets (ESP32, Pico).

## Decision

We adopt a strict **hexagonal (ports & adapters) layout**:

- `booth-core` owns the state machine. Pure functions. No IO. No clock.
- `booth-hal` defines traits (`GpioPort`, `AudioSink`, …) — the "ports".
- `booth-pi`, `booth-mock`, future `booth-esp32` etc. are the "adapters"
  that implement those traits for a particular platform.
- `booth-bin` is the glue: configure, instantiate adapters, run the
  state machine, host the debug surface.

## Consequences

**Good:**

- Every state transition has a unit test that runs in milliseconds on a
  laptop.
- Adding a new platform doesn't touch the core or the existing adapters.
- The HAL trait list itself documents what a port actually needs to
  provide.

**Trade-offs:**

- More crates, more `Cargo.toml`s — slight upfront friction.
- Trait objects (`Box<dyn AudioSink>`) cost a vtable dispatch per call,
  which is irrelevant at our scale but worth knowing.
