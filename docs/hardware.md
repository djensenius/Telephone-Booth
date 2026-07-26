# Hardware

The Rust client targets a Raspberry Pi (any model with the 40-pin header) with
a **USB-Audio-Class 2.0** audio interface plugged in — a Focusrite Scarlett
Solo / 2i2 is the reference device, but any UAC2 interface should work.

## Rotary phone wiring

The reference booth is built from a vintage **three-slot coin payphone**
(Western Electric / Northern Electric, `233`-type network, `P-13E961` rotary
dial). The telephone network and coin mechanism are **not used** — the
client bypasses them entirely. You only tap three switch contacts for GPIO and
the two handset capsules for audio (see
[Handset transmitter and receiver](#handset-transmitter-and-receiver)).

For a full subsystem breakdown of the physical phone — the network block, coin
relay, signal gong, terminal designations, and links to the original `233G`
service manuals — see [`payphone-reference.md`](payphone-reference.md).

The booth uses three GPIO inputs against ground, debounced in software:

| Function       | Default BCM pin | Physical pin | Wire color (typical) |
| -------------- | --------------- | ------------ | --------------------- |
| Hook switch    | BCM 17          | 11           | green                 |
| Rotary pulse   | BCM 27          | 13           | yellow                |
| Rotary gate (off-normal) | BCM 22 | 15           | blue                  |

Ground is physical pin 9 (any GND pin on the header works).

All three inputs are configured with the Pi's internal pull-up resistor by
default (`rppal` `PullUp`, overridable to `PullDown`) and read **active-low**
when wired as contacts to ground (closed = 0). The runtime maps each pin's
debounced logical level to a `booth-core` event:

- **Hook switch** — level high → `HookOn` (handset resting / idle); level low →
  `HookOff` (handset lifted). Tap the switchhook leaf contacts in the upper
  housing.
- **Rotary pulse** — the dial's *impulse* contacts, which open once per click as
  the finger wheel returns. Each break (opening) edge is emitted as `RotaryPulse`
  after a 25 ms debounce. Because the impulse contact is normally closed, the
  default `invert.rotary_pulse = true` counts these break pulses. Pulses are
  counted and the digit is decoded after a 350 ms idle gap
  (`PULSE_GROUP_TIMEOUT_MS`): 1–9 pulses → that digit, 10 pulses → `0`, more than
  10 → the group is discarded and the booth returns to dial tone.
- **Rotary gate (off-normal)** — the dial's *off-normal* / shunt contacts, which
  stay closed while the wheel is away from rest. The current runtime **reads
  this pin for the debug pin matrix and telemetry but does not use it to decode
  digits** (`event_from_gpio` returns `None` for `RotaryRead`); decoding relies
  on the pulse count plus the 350 ms timeout above. Wiring it is therefore
  optional — handy for debugging, not required to dial. (The legacy Node.js
  client *did* close each digit on this contact's trailing edge; the Rust client
  deliberately does not.)

Only **hook** and **pulse** are functionally required. Because polarity depends
on which leaf of each contact you tap, bring the booth up with the
[debug pin matrix](debug-panel.md) open, watch the live levels while you lift the
handset and dial, and if any signal reads inverted flip `gpio.pull` or
`gpio.invert.<role> = true` — see [`configuration.md`](configuration.md). The
ignored `booth-pi` loopback test documents a hardware smoke test using an output
pin wired to one of these inputs.

### Pin mapping defaults

The defaults (hook → BCM 17 / physical 11, rotary pulse → BCM 27 / physical 13,
rotary gate → BCM 22 / physical 15) are the recommended wiring for a fresh
build. Every pin is overridable:

```toml
# /etc/phone-booth/config.toml
[gpio]
hook_bcm         = 17
rotary_pulse_bcm = 27
rotary_gate_bcm  = 22       # alias: rotary_read_bcm; optional (see above)
pull             = "up"     # or "down"
debounce_ms      = 25
```

### Reusing a legacy Node.js booth harness

> **Heads-up:** the Rust defaults do **not** match the original Node.js wiring.
> An earlier version of this page claimed they did — they don't. The hook and
> gate wires are swapped between the two.

The legacy Node.js client (`legacy-node-v1` tag) addressed the header by
**physical** pin number and assigned different roles:

| Physical pin | Legacy Node.js role       | Rust default role        |
| ------------ | ------------------------- | ------------------------ |
| 11           | Rotary gate ("channel")   | **Hook switch** (BCM 17) |
| 13           | Rotary pulse              | Rotary pulse (BCM 27)    |
| 15           | Hook switch ("hangupper") | **Rotary gate** (BCM 22) |

Pulse (pin 13) matches, but hook and gate are reversed. If you re-flash an
existing booth **without** re-soldering its harness, the old hook wire lands on
the (ignored) gate pin and hook detection silently fails. Either move the two
wires, or keep the harness and remap the roles in config:

```toml
# Reuse a legacy Node.js harness unchanged:
[gpio]
hook_bcm         = 22       # physical 15 — where the legacy hook wire already is
rotary_pulse_bcm = 27       # physical 13 — unchanged
rotary_read_bcm  = 17       # physical 11 — legacy gate wire (read-only anyway)
pull             = "up"
```

### GPIO screw terminal HAT (optional but recommended)

Soldering directly to the 40-pin header is fiddly and unforgiving inside a
booth. A **GPIO screw-terminal breakout HAT** — the reference build uses the
52Pi *GPIO Screw Terminal HAT* (`SKU EP-01129`) — makes the phone leads
tool-free to land and easy to re-seat.

It is a **pure passthrough**: every screw terminal is one standard 40-pin BCM
GPIO, brought straight out with no remapping. Your pin assignments (and the
config keys below) are therefore **unchanged** — you just screw each wire into
the terminal whose silkscreen matches the BCM number instead of soldering to a
header pin. Each terminal has an LED indicator beside it that follows the pin's
level, which is handy for eyeballing the contacts while you wire.

Landing the reference wiring on the HAT:

| Phone lead               | Screw terminal (BCM silkscreen) | Config key                     |
| ------------------------ | ------------------------------- | ------------------------------ |
| Hook switch              | `IO17`                          | `hook_bcm = 17`                |
| Rotary pulse             | `IO27`                          | `rotary_pulse_bcm = 27`        |
| Rotary gate (off-normal) | `IO22` (optional)               | `rotary_gate_bcm = 22`         |
| Ground (common return)   | any `GND`                       | —                              |

Bring the booth up with the [debug pin matrix](debug-panel.md) open and watch
both the on-board LEDs and the live software levels as you lift the handset and
dial. If a signal reads inverted, flip `gpio.pull` or set
`gpio.invert.<role> = true` — see [`configuration.md`](configuration.md).

For a full-screen console dashboard directly on the Pi (rather than the web
pin matrix), either:

- stop the service and run the read-only hardware monitor:
  `sudo systemctl stop telephone-booth && sudo -u phonebooth /usr/bin/telephone-booth run --tui`; or
- keep the service running and attach passively over the debug surface:
  `sudo -u phonebooth BOOTH_DEBUG_TOKEN=... /usr/bin/telephone-booth run --tui --attach https://127.0.0.1:8443`.

See [`simulator.md`](simulator.md#read-only-hardware-monitor---tui) and
[`simulator.md`](simulator.md#monitor-vs-web-pin-matrix).

### As-built dial + hook wiring (reference booth)

This records the actual, meter-verified wiring of the reference booth's
`P-13E961` dial and switchhook, and how the four leads are carried to the Pi
over a **second, dedicated Ethernet cable** (separate from the audio + 5 V run).

**Contact decode (verified with a multimeter):**

| Wire  | Phone terminal | Continuity behaviour (meter)                          | Function            |
| ----- | -------------- | ----------------------------------------------------- | ------------------- |
| blue  | **W**          | shared common across *both* contact sets              | common → ground     |
| red   | **BB**         | closed at rest, opens while dialling                  | rotary pulse (NC)   |
| green | —              | open at rest, closed *only* while wheel is off rest   | rotary gate (off-normal) |
| white | —              | switchhook leaf contact                               | hook                |

- **blue is the common.** It reads through both the pulse pair (blue+red) and
  the gate pair (blue+green), so it is the shared node that lands on Pi `GND`.
- **red = pulse** — blue+red beeps at rest and opens as the wheel returns.
- **green = gate** — blue+green beeps *only* while the wheel is off its rest
  position.
- **white = hook.** Its return shares the single common (blue) ground rail.

**Landing on the Pi over the second Ethernet cable (T568B colours):**

| Wire  | Function          | Ethernet conductor | Pi-side termination            |
| ----- | ----------------- | ------------------ | ------------------------------ |
| white | Hook              | blue               | `IO17` (`hook_bcm = 17`)       |
| red   | Pulse             | orange             | `IO22` — see config note below |
| green | Gate (optional)   | green              | `IO27` — see config note below |
| blue  | Common → ground   | white-blue         | any `GND`                      |

> **Non-default pin mapping.** The reference booth lands **pulse (red) on IO22**
> and **gate (green) on IO27** — the *opposite* of the code defaults
> (`rotary_pulse_bcm = 27`, `rotary_gate_bcm = 22`). You **must** swap the config
> to match, or dialling will not decode:
>
> ```toml
> [gpio]
> hook_bcm         = 17   # white
> rotary_pulse_bcm = 22   # red   — pulse contact
> rotary_gate_bcm  = 27   # green  — gate (off-normal), optional
> ```

Only **hook** (white) + **pulse** (red) + **ground** (blue) are required to
dial; **gate** (green) is optional debug telemetry. Bring the booth up with the
[debug pin matrix](debug-panel.md) open and dial `0` — you should see **10
pulses** on the pulse pin. If pulse reads inverted, flip
`gpio.invert.rotary_pulse`.

## Handset transmitter and receiver

The mouthpiece **transmitter** and the earpiece **receiver** are two *different*
elements, and both are wired to the **audio interface**, not to GPIO:

- the **transmitter** is the *microphone* — it feeds the interface's input;
- the **receiver** (earpiece) is the *speaker* — it is driven from the
  interface's headphone / line output.

On a vintage handset both are removable capsules under the screw-off caps. You
do not need the phone's `233`-type network for either — run two wires from each
capsule straight to the interface.

### Transmitter (microphone) options

Vintage handsets use a **carbon transmitter** (e.g. Western Electric `T1`): a
capsule of carbon granules whose resistance varies with sound pressure. It needs
a DC bias current to work at all, is electrically noisy and low-fidelity, drifts
as the granules pack, and will not plug straight into a modern mic input. In
rough order of audio quality (and increasing departure from "all original"):

1. **Swap in an electret capsule** (recommended). Remove the carbon button and
   drop a small electret microphone into the cap. Power it from a mic input that
   supplies plug-in bias, or from a tiny electret preamp module (e.g. `MAX9814`,
   Adafruit electret amp) feeding a line input. Cleanest result for the least
   money, and what most booth rebuilds do.
2. **Fit a dynamic element** into the `XLR` / mic input. No bias needed, robust,
   good quality; the capsule is larger so it may need creative mounting.
3. **Buy a drop-in replacement capsule.** Reproduction transmitter elements sold
   for vintage phones are pin-compatible and self-contained (usually electret
   inside), so they work without external bias — near plug-and-play.
4. **Keep the original carbon element and bias it** (most authentic, lo-fi).
   Feed it ~3–9 V DC through a current-limiting resistor and couple the audio out
   through a `~600:600 Ω` line transformer (or a DC-blocking capacitor) into a
   line input. Expect hiss and the occasional "tap the handset to wake it up".
5. **Replace the handset guts with a USB / VoIP handset module** (most reliable,
   least authentic) — a fallback if the period element does not matter to you.

Set the interface input gain per [Microphone level](#microphone-level) once the
element is chosen.

> **Reference-booth note (carbon on a USB dongle).** In practice the original
> carbon element passed usable, intelligible voice on a generic C-Media USB
> dongle's **plug-in bias alone** — no external bias circuit (option 4) — once
> the capture gain was pushed near the top of its range (~+17 dB) and the
> dongle's **Auto Gain Control was turned off** (AGC pumps the noise floor up
> on a quiet carbon source). It is still lo-fi and level varies as the granules
> pack, but it works. Those mixer levels are now applied automatically at
> startup via the [`[audio.mixer]`](configuration.md#startup-alsa-mixer) config
> block so they survive reboots.

### Receiver (earpiece) quality and level

The receiver is a *separate*, low-sensitivity element with a deliberately narrow
(telephone-band, ~300–3400 Hz) response — that "small and tinny" timbre is
period-correct, not a fault. Most vintage receivers are a few tens to a few
hundred ohms, and a UAC2 headphone output (designed for 16–300 Ω loads) can
drive them **directly**, just quietly. To dial in level and quality:

- **Direct drive** (simplest): wire the receiver to the headphone / line out and
  raise the level in `alsamixer` or the OS mixer. Add a small series resistor (a
  few hundred ohms) if it is too loud or to protect a fragile coil.
- **Add a small mono amplifier** (e.g. `PAM8302`, `LM386`) between a line out and
  the receiver if direct drive is too quiet; tame the output with a series
  resistor or an L-pad so the interface is not run at full tilt.
- **Replace the receiver element** with a modern 8–32 Ω mini speaker / driver for
  louder, fuller sound — at the cost of authenticity.
- **Shape the audio at the source.** Because the booth plays fixed clips, the
  most reliable EQ is baked into the clips: a gentle band-pass / presence lift
  around 300–3400 Hz plus a high-pass to kill rumble maximizes intelligibility on
  a tiny element without fighting ALSA.

Keep playback levels modest into an original receiver — a high-power speaker amp
can cook a vintage voice-coil.

## Running audio + 5 V to the booth over one Ethernet cable

The handset audio and a 5 V supply for any in-booth electronics (an electret mic
preamp, a small receiver amp, etc.) can all share a single **Cat5e/Cat6**
run between the electronics box and the phone. Cat cable is four **twisted
pairs** (eight conductors); the twist is what rejects hum and crosstalk, so the
schema below keeps each audio signal in its *own* pair alongside a ground
return, and gives 5 V its own pair too. That "audio-first" layout stops power
return current from sharing an audio ground.

Five solid AWG leads land on the booth side: `green` ×2 (ground), `red` (5 V),
`blue` (**T** — receiver / audio *out*), and `black` (**TR** — transmitter /
audio *in*). Map them to the Ethernet conductors by **T568B** colour:

| Booth lead (AWG)      | Function                | Ethernet pair | Ethernet conductor (T568B) | RJ45 pin |
| --------------------- | ----------------------- | ------------- | -------------------------- | -------- |
| `blue`                | **T** — receiver (out)  | Pair 1 (blue) | blue                       | 4        |
| `green` #1            | ground (return for T)   | Pair 1 (blue) | white-blue                 | 5        |
| `black`               | **TR** — transmitter (in) | Pair 3 (green) | green                    | 6        |
| `green` #2            | ground (return for TR)  | Pair 3 (green) | white-green                | 3        |
| `red`                 | 5 V                     | Pair 2 (orange) | orange                   | 2        |
| — (bond to GND bus)   | 5 V return              | Pair 2 (orange) | white-orange             | 1        |
| — (spare)             | spare / power parallel  | Pair 4 (brown) | white-brown, brown         | 7, 8     |

Notes:

- **Each audio line gets a full twisted pair.** `blue`/**T** rides the blue pair
  with a ground on its partner conductor; `black`/**TR** rides the green pair the
  same way. The twist keeps the mic and earpiece signals quiet.
- **5 V has its own pair.** You only have two `green` ground leads (both used as
  audio returns), so the 5 V return is the orange pair's `white-orange`
  conductor — tie it into the common ground bus at **both** ends. That gives the
  power a twisted return without borrowing an audio ground.
- **Long run or a hungry 5 V load?** Parallel the spare brown pair with the power
  pair — `brown` + `orange` = 5 V, `white-brown` + `white-orange` = ground — to
  roughly halve the supply resistance and voltage drop.
- **Keep the RJ45 shell / drain (if the cable is shielded) on ground at one end
  only** to avoid a ground loop.
- Confirm polarity and continuity end-to-end with a multimeter before powering
  up; T/TR are defined in
  [Handset transmitter and receiver](#handset-transmitter-and-receiver).

### Landing it on the Pi side (USB dongle build)

The reference build terminates the audio leads on a **generic USB audio dongle**
with two 3.5 mm breakouts — one **mic** jack and one **speaker** jack, each
broken out to L / R / ground. The dongle is mono per capsule and its two jacks
share a common internal ground, so the two `green` returns simply land on each
jack's sleeve:

| Booth lead            | Ethernet conductor | Pi-side termination                     |
| --------------------- | ------------------ | --------------------------------------- |
| `blue` — **T** (out)  | blue               | Speaker breakout **tip** (L)            |
| `green` #1 (T return) | white-blue         | Speaker breakout **ground / sleeve**    |
| `black` — **TR** (in) | green              | Mic breakout **tip** (L)                |
| `green` #2 (TR return)| white-green        | Mic breakout **ground / sleeve**        |
| `red` — power         | orange             | Pi header 3.3 V (phys 1) — see note     |
| power return          | white-orange       | Pi header `GND` (phys 6/9/14/…)         |
| spare                 | brown pair         | leave unterminated                      |

> **Power-rail note.** The schema labels the `red` lead "5 V", but the reference
> booth actually drives it from the Pi header's **3.3 V** rail (physical pin 1)
> — enough for electret plug-in bias and similar low-draw needs. If a future
> load genuinely needs 5 V, move `red` to physical pin 2/4 and re-check the
> load's current against the Pi's shared 5 V budget. Either way the USB dongle
> powers itself over USB; this lead is only for auxiliary in-booth electronics.

### As-built MAX9814 mic wiring

The working booth uses a `MAX9814` electret preamp in the handset path. The
final, verified terminations are:

| Signal / terminal      | Wires to                                              |
| ---------------------- | ----------------------------------------------------- |
| **TR**                 | Microphone (mic element)                              |
| **L** (loopback)       | Audio loopback into the `MAX9814` **mic inputs**      |
| **V+**                 | Pi header **3.3 V**                                   |
| **Pi GND** (star node) | Both the **audio ground** *and* the **3.3 V ground**  |

The single Pi ground is the common star-ground node: the audio return and the
3.3 V supply return both land on it, which is what finally killed the floating-
ground hum during bring-up. Keep the mic run as short as practical to the
preamp to minimise mains pickup.

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

On a **generic USB dongle** there is no analog gain wheel — the capture
level and switches live in the ALSA mixer (`amixer -c <card>`). Rather than
tuning them by hand and persisting with `alsactl store`, let the booth set
them deterministically at startup via the
[`[audio.mixer]`](configuration.md#startup-alsa-mixer) config block. For the
reference dongle + carbon mic that means raising the `Mic` capture control
near the top of its range (~83 %) and disabling `Auto Gain Control`.

### Recording format

All recordings are **FLAC** (lossless, mono, 48 kHz) — see
[ADR 0003](adr/0003-flac-as-recording-format.md). Files are stored at
`/var/lib/phone-booth/recordings/<sha256>.flac` and uploaded to Azure
Blob Storage via a presigned SAS URL.

## Power & boot

The Pi should boot off an SD card (or, preferably, an SSD via USB3) running
Raspberry Pi OS 64-bit. The systemd unit installed by the `.deb` package
waits for `network-online.target` so the client never tries to contact the
operator before networking is up. See [`packaging.md`](packaging.md).
