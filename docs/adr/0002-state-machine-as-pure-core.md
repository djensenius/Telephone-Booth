# ADR 0002 — State machine as a pure core

**Status:** accepted.

## Context

Phone behavior is inherently a state machine: off-hook plays a dial tone,
dialing a digit kicks off a fetch, beep precedes recording, etc. We want
the rules to be obvious from reading the code and exhaustively testable
without hardware.

## Decision

The state machine is a pure function:

```rust
pub fn handle(state: State, event: Event) -> (State, Vec<Effect>)
```

- `State` is an `enum` of every state the booth can be in.
- `Event` is everything that can happen (hook on/off, pulse, tick,
  upload finished, playback ended, …).
- `Effect` is everything the booth can _do_ (`Play`, `Stop`,
  `StartRecording`, `Upload`, `FetchRandomQuestion`, …).

The runtime is the thing that translates effects into HAL calls and turns
HAL events back into machine events.

## Consequences

**Good:**

- The full transition table is one big `match`. New states / events /
  effects need a single edit + a unit test.
- We can snapshot transitions with `insta` and visually diff regressions.
- A debug-mode endpoint can replay a previous sequence of events and get
  identical output, which is invaluable for bug reports.

**Trade-offs:**

- Effects must be totally describable as data. Anything that needs
  ad-hoc runtime parameters (e.g. an arbitrary URL) needs to be modeled
  explicitly. See the `Effect::Play(AudioRef)` discussion in
  `docs/architecture.md`.
