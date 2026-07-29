# ADR 0009 — Status LED and power button port

**Status:** accepted.

## Context

Issue #135 asks for a physical power/reset control and an at-a-glance status
indicator on the booth. The chosen part is an **Adafruit 3350** illuminated
pushbutton: a momentary, normally-open switch plus an RGB LED ring with a
common anode.

Two hardware facts, bench-verified on the reference Pi 4, constrain the design:

- **The LED ring shares a single current-limiting resistor across all three
  cathodes**, not one per colour. When two or more cathodes are driven low at
  once, only the colour with the lowest forward voltage lights (red beats
  green, green beats blue). Colour mixing is therefore physically impossible.
  Time-multiplexing the channels to fake mixed colours was tested and rejected
  as visibly unstable.
- **Waking a halted Pi 4 requires BCM 3 (I2C1 SCL)**, which is already used by
  the AudioInjector Flatmax HAT's audio codec. That pin is unavailable, so the
  button can reboot and power off the booth but cannot power a halted booth
  back on.

The architecture is hexagonal (ADR 0001) with a pure core (ADR 0002): the core
must not perform I/O, read a clock, or depend on hardware. Any new capability
has to be expressed as an `Effect` data value executed by the runtime through a
HAL trait.

## Decision

Add a status-LED and power-button port, keeping the core pure.

- **`booth-hal`** gains a `StatusLed` trait (`async fn set(colour, pattern)`)
  and a `PowerController` trait (`reboot` / `poweroff`), matching the shape,
  error style, and async signature of the existing HAL ports.
- **`LedColour` is `Off | Red | Green | Blue`**, deliberately *not* an RGB
  triple. Because the hardware cannot mix colours, an unmixable colour is made
  unrepresentable in the type system. `LedPattern` carries the timing
  (`Steady`, `Pulse`, `Blink`, `Fade`).
- **The Pi adapter drives at most one cathode low at a time** and holds the
  other two high (never floating), via software PWM (hardware PWM channels
  collide with I2S). This enforces the shared-current-limit constraint in code.
- **The core** emits `Effect::SetStatusLed { colour, pattern }` on every state
  transition (mapping in `booth_core::status_led_for`), plus `Effect::Reboot`
  and `Effect::PowerOff` for the button. `Event::PowerButtonPressed` maps to
  reboot; `Event::PowerButtonHeld` maps to power-off. The runtime does all
  press-duration timing; the core never reads a clock.
- **Both features are opt-in and default-off**, wired through config
  (`[power_button]`, `[status_led]`) and env overrides, so existing deployed
  booths are unaffected on upgrade.
- **The runtime is the single publisher of `TelemetryEvent::StatusLed`.**
  Adapters only drive hardware; the runtime publishes once per accepted change
  so every backend (Pi, mock, no-op) reports identically and the transient
  boot / ready / shutdown indications — which are not core states — are visible
  to the debug surface too.
- **The `phonebooth` service user is authorized for exactly two logind
  actions** (`reboot`, `power-off`) via a packaged polkit rule, rather than
  granting the service broader privileges or running it as root.
- **Power-button edges fail safe under backpressure rather than being
  dropped.** The shared GPIO edge queue drops edges when full, which is fine for
  hook / pulse but not here: losing a release would make a short press
  indistinguishable from a hold and trigger an unintended power-off. The Pi
  poller instead retries an undeliverable button edge on the next 2 ms tick,
  keeping only the newest level. This is *not* lossless — if the queue stays
  full across both edges of a short press, the retained release supersedes the
  press and the requested reboot is simply never emitted — but the failure mode
  is "nothing happens", never "the booth powers off". For the same reason the
  runtime's `gpio_task` hands hook / rotary events to a forwarder task through
  a second bounded queue, so a stalled core cannot block edge intake and delay a
  release past `hold_ms`. That queue provides no stronger delivery guarantee
  than the Pi one: when it fills — which needs the core wedged for both queue
  depths — hook / rotary events are dropped and counted
  (`booth_gpio_events_dropped_total`) rather than allowed to grow without
  bound. Power-button signals never travel on it. Finally,
  `power_button_task` re-checks for a queued release when the hold timer
  fires.

## Consequences

**Good:**

- The impossible-to-mix hardware reality is encoded in the type system, so no
  code path can request an unlit or contradictory colour combination.
- The core stays pure and fully testable: the State → (colour, pattern) mapping
  is snapshot-tested with `insta`, and a `proptest` random walk asserts the LED
  is never left in an undefined state and never drives two channels at once.
- Reboot / power-off is testable without touching real `systemctl` because the
  `PowerController` port is mocked in integration tests; the real `systemctl`
  call lives only in the Pi adapter.

**Trade-offs:**

- A `PowerController` HAL port was added beyond the literal brief (which said to
  invoke `systemctl` directly in the binary) so the reboot/power-off path is
  mockable in integration tests. The actual `systemctl reboot` / `systemctl
  poweroff` invocation still lives in the Pi adapter.
- Button hold timing runs against the runtime's async timers rather than a
  separately injected `Clock`, mirroring the existing pulse-timeout handling;
  the core remains clock-free.
- The button cannot power a halted Pi 4 back on (BCM 3 is taken by the audio
  codec). Operators who need a physical power-on must fit an inline PSU switch;
  this is documented in `hardware.md`.
- Software PWM on three GPIOs adds a small, bounded CPU cost on the Pi; within
  budget for the booth's workload.
- The reboot / power-off path depends on polkit being installed and on the
  packaged rule being present. Running the binary outside the `.deb` (or as a
  different user) requires installing that rule by hand; this is documented in
  `hardware.md`.
