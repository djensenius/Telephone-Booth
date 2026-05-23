# Architecture

The Rust client is a small **hexagonal / ports-and-adapters** application.
The phone's behavior lives in a pure state machine that knows nothing about
GPIO, audio, HTTP, or files. Everything the machine wants to _do_ is a
declarative `Effect` that the runtime translates into HAL calls.

```mermaid
flowchart LR
  subgraph Core
    SM[booth-core\nstate machine]
  end
  subgraph HAL traits
    H1[GpioPort]:::trait
    H2[AudioSink]:::trait
    H3[AudioSource]:::trait
    H4[OperatorClient]:::trait
    H5[Clock]:::trait
    H6[Storage]:::trait
  end
  subgraph Adapters
    PI[booth-pi\nrppal / cpal / reqwest]
    MK[booth-mock\nin-memory]
    F32[(future)\nbooth-esp32]
  end
  subgraph Edge
    OP[Operator HTTPS]
    DB[booth-debug\naxum + htmx]
  end
  PI --> H1 & H2 & H3 & H4 & H5 & H6
  MK --> H1 & H2 & H3 & H4 & H5 & H6
  F32 -.-> H1 & H2 & H3 & H5
  H1 & H2 & H3 & H4 & H5 & H6 --> SM
  SM <-->|telemetry bus| DB
  H4 --> OP
  classDef trait fill:#fff,stroke:#999,stroke-dasharray:3 3;
```

## Crates

| Crate              | Responsibility                                                                                                  |
| ------------------ | --------------------------------------------------------------------------------------------------------------- |
| `booth-hal`        | Trait definitions (`GpioPort`, `AudioSink`, …) + shared types (`AudioRef`, `SystemSnapshot`, `CallOutcome`, …) |
| `booth-core`       | Pure state machine: `fn handle(state, event) -> (state, Vec<Effect>)`                                           |
| `booth-mock`       | In-memory HAL adapters for unit tests and host-side runs                                                        |
| `booth-pi`         | Pi adapter — `rppal` GPIO, `cpal` audio, `reqwest` HTTP                                                         |
| `booth-telemetry`  | In-process broadcast bus for `TelemetryEvent` records (used by `booth-debug`, `booth-metrics`, and the forwarder) |
| `booth-metrics`    | Periodic `SystemSnapshot` sampler + Prometheus `MetricsHandle` (gauges/counters/histograms) updated from the bus |
| `booth-debug`      | Embedded debug HTTP server (`axum` + `tokio::sync::broadcast` + htmx UI). Loopback also serves `/metrics`.       |
| `booth-bin`        | Wires it all together: tokio runtime, config loader, signal handling, session tracker, event forwarder.        |

## State machine

States: `Idle`, `DialTone`, `Dialing { pulses }`, `PlayingQuestion`, `Beep`,
`Recording`, `Uploading { recording_id }`, `PlayingMessage`,
`PlayingInstructions`, `Error { reason }`.

Events the runtime feeds in: `HookOn`, `HookOff`, `RotaryPulse`,
`DigitClosed(u8)`, `PlaybackEnded`, `RecordingFinished`, `UploadComplete`,
`UploadFailed`, `Tick`.

Effects the runtime executes: `Play(AudioRef)`, `Stop`, `StartRecording`,
`StopRecording`, `Upload`, `FetchRandomQuestion`, `FetchRandomMessage`,
`PutStatus(BoothStatus)`, `ArmPulseTimeout`.

Pulses 1..=9 map to themselves; **10 pulses = digit 0**. More than 10 pulses
in a single group resets to `DialTone`. A pulse group is closed by `Tick`
after `PULSE_GROUP_TIMEOUT_MS = 350` ms.

Digit 1 fetches a random question; digit 2 fetches a random message;
digits 3..=9 and 0 play the operator-recorded `Instructions` audio.

See [`debug-panel.md`](debug-panel.md) for the live telemetry stream the
state machine drives.

## Telemetry bus

Every layer publishes `TelemetryEvent` records onto an in-process
`tokio::sync::broadcast` channel. The same stream feeds:

- the `tracing` subscriber (stdout / journal)
- the `/debug/stream` WebSocket
- a 4096-slot ring buffer for client catch-up on reconnect

## Threading model

The runtime is `tokio` multi-threaded. The state machine runs on a single
task that owns the canonical `State`; events arrive over an `mpsc` channel.
Effects are dispatched to a small pool of adapter tasks (audio, operator
HTTP, GPIO) so a slow upload never blocks the next rotary pulse.
