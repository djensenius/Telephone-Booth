# Hardware

The Rust client targets a Raspberry Pi (any model with the 40-pin header) with
a **USB-Audio-Class 2.0** audio interface plugged in — a Focusrite Scarlett
Solo / 2i2 is the reference device, but any UAC2 interface should work.

## Information card

The information card measures **100 mm wide × 87 mm high**. The repository
includes an editable [`SVG`](reference/information-card.svg) and a print-ready
[`PDF`](reference/information-card.pdf) using Univers Bold and Univers
Condensed.

## Noctua PWM cooling fan

The reference Pi 4 cooling setup uses the **Noctua NF-A4x20 5V PWM**. These
instructions apply to that 5 V model, not the visually similar 12 V version.
The fan draws at most 0.12 A; use a Pi power supply with enough headroom for
the fan, USB audio interface, and other attached hardware.

### Fan wiring

Shut the Pi down and disconnect power before touching the header:

| Fan lead | Fan connector pin | Pi physical pin | Pi function |
| -------- | ----------------- | --------------- | ----------- |
| black    | 1                 | 6               | ground      |
| yellow   | 2                 | 4               | 5 V         |
| green    | 3                 | not connected   | tachometer   |
| blue     | 4                 | 12              | GPIO18 / PWM0 |

The standard fan connector order is ground, power, tachometer, PWM. Do not
identify the ends solely from a phrase such as "keyed tab facing you": the
result changes depending on whether the mating face or wire-entry face is in
view. Confirm the wire colours and connector pin numbers against the fan
datasheet.

The green tachometer lead must be insulated individually so it cannot touch
the header or enclosure. No resistor is needed while that wire is disconnected.
The Noctua PWM input has its own pull-up and supports direct CMOS GPIO drive,
so the blue lead connects directly to GPIO18 without an external pull-up,
series resistor, level shifter, or transistor. Never connect the yellow 5 V
lead to a GPIO.

Use separate female-to-female jumpers or a keyed breakout rather than forcing
the fan's four-pin shroud onto the Pi header. Secure the jumpers against
vibration, but keep each connection removable and insulated. The NA-RC7
Low-Noise Adaptor is optional; PWM control already reduces speed dynamically,
and the adaptor only caps the maximum.

Before applying power, check the three physical pin numbers again. Physical
pin 4 is 5 V; physical pin 8 is a GPIO and will be damaged by this connection.
On power-up the fan runs at full speed until Linux claims GPIO18. That
fail-safe behaviour is expected.

### Install the 25 kHz thermal overlay

Noctua specifies a 25 kHz PWM target (21–28 kHz accepted). Raspberry Pi OS's
stock `pwm-gpio-fan` overlay uses 50 Hz, so do not use it for this four-wire
fan. The repository ships a hardware-PWM overlay at
`packaging/raspberry-pi/noctua-pwm-fan-overlay.dts`.

On the Pi, from a checkout of this repository:

```sh
sudo apt-get update
sudo apt-get install -y device-tree-compiler
sudo install -D -m 0644 packaging/raspberry-pi/noctua-pwm-fan-overlay.dts \
  /usr/local/share/telephone-booth/noctua-pwm-fan-overlay.dts
sudo dtc -@ -I dts -O dtb \
  -o /tmp/noctua-pwm-fan.dtbo \
  /usr/local/share/telephone-booth/noctua-pwm-fan-overlay.dts
sudo install -m 0644 /tmp/noctua-pwm-fan.dtbo \
  /boot/firmware/overlays/noctua-pwm-fan.dtbo
sudo cp -p /boot/firmware/config.txt \
  /boot/firmware/config.txt.pre-noctua-fan
sudo editor /boot/firmware/config.txt
```

The Debian package also installs the source as
`/usr/share/telephone-booth/noctua-pwm-fan-overlay.dts`; use that path instead
when no repository checkout is present.

GPIO18/PWM0 conflicts with the Pi's onboard analog headphone output. The booth
uses USB audio, so disable only the onboard device and load the custom overlay:

```ini
[all]
dtparam=audio=off
dtoverlay=noctua-pwm-fan
```

Keep any existing settings in `config.txt`, then reboot:

```sh
sudo reboot
```

The overlay installs this temperature curve:

| CPU temperature while heating | PWM command | Cooling state |
| ----------------------------- | ----------- | ------------- |
| below 50°C                    | off         | 0             |
| 50–59.9°C                     | 25%         | 1             |
| 60–67.4°C                     | 40%         | 2             |
| 67.5–74.9°C                   | 65%         | 3             |
| 75°C and above                | 100%        | 4             |

Each threshold has 5°C hysteresis. For example, the fan starts at 50°C but
does not stop again until the Pi cools below 45°C. It is therefore normal for
the fan to be completely stopped at idle.

### Verify fan control

After reboot, GPIO18 must be assigned to PWM0 and a `pwm-fan` cooling device
must exist:

```sh
pinctrl get 18
grep -H . /sys/class/thermal/cooling_device*/type
grep -H . /sys/class/thermal/cooling_device*/cur_state
grep -H . /sys/class/hwmon/hwmon*/name
grep -H . /sys/class/hwmon/hwmon*/pwm1 2>/dev/null
cat /sys/class/thermal/thermal_zone0/temp
```

`pinctrl` should report `PWM0_0`. The `pwmfan` hardware-monitor device reports
the current command from 0 to 255. CPU temperature is in millidegrees Celsius.
With the green tachometer wire disconnected, software can report the requested
PWM duty but cannot prove that the rotor is moving or measure RPM.

The booth publishes the same values through `GET /v1/system` and Prometheus;
see [`observability.md`](observability.md#fan-monitoring).

To roll back, restore the saved configuration and reboot:

```sh
sudo cp /boot/firmware/config.txt.pre-noctua-fan \
  /boot/firmware/config.txt
sudo reboot
```

## Cable and connector conventions

**This project terminates every 8P8C / RJ45 connector to `T568B`.** That applies
to real network drops *and* to the Cat5e/Cat6 runs this build repurposes as
multi-conductor cable for GPIO, handset audio, and 5 V power. Every wiring table
in this document names conductors by their T568B colour.

T568B pin order, pin 1 → 8 (holding the plug with the contacts facing you, the
latch pointing down, and the cable running away from you, pin 1 is on the left):

| Pin | Conductor    | Pair            |
| --- | ------------ | --------------- |
| 1   | white-orange | Pair 2 (orange) |
| 2   | orange       | Pair 2 (orange) |
| 3   | white-green  | Pair 3 (green)  |
| 4   | blue         | Pair 1 (blue)   |
| 5   | white-blue   | Pair 1 (blue)   |
| 6   | green        | Pair 3 (green)  |
| 7   | white-brown  | Pair 4 (brown)  |
| 8   | brown        | Pair 4 (brown)  |

Notes:

- **Both ends get T568B**, giving a straight-through cable. Terminating one end
  T568A and the other T568B yields a crossover, which will silently break the
  pair assignments in the audio and power tables below.
- **T568A and T568B are electrically identical** — the choice is pure
  convention. T568B is the prevailing convention in North America, so it is what
  this build standardises on. Do not mix the two within the booth.
- The only difference is that the orange and green pairs swap places. Pins 4, 5,
  7, and 8 are the same in both schemes.
- Pin numbers used in the wiring tables below are RJ45 pin numbers under this
  scheme. The
  [audio + 5 V run](#running-audio--5-v-to-the-booth-over-one-ethernet-cable)
  puts each handset signal on its own twisted pair (blue and green) and gives
  5 V the orange pair, so that power return current never shares an audio
  ground.

### The booth runs are not Ethernet — never patch them into a network

Two of the cables described in this document are Cat5e/Cat6 with 8P8C plugs on
both ends, but carry **DC power, unbalanced analog audio, and switch contacts**
rather than Ethernet: the
[audio + 5 V run](#running-audio--5-v-to-the-booth-over-one-ethernet-cable) and
the [dial + hook run](#as-built-dial--hook-wiring-reference-booth). They are
physically indistinguishable from a patch lead. Treat them as a hazard:

- **Never plug a booth run into a switch, NIC, router, or patch panel**, and
  never plug a real network cable into a booth-side jack. Pins 1–2 and 3–6 are
  differential signal pairs on real Ethernet; a booth run puts a DC supply and
  GPIO contacts across them. That can destroy the Pi, the supply, and the
  network device at once.
- **A PoE port is worse.** PoE will push up to 48 V down the cable into GPIO
  pins and audio inputs rated for 3.3 V.
- **Label both ends unmistakably** and keep the booth runs physically separated
  from any real network drop. A distinct cable colour, or a non-RJ45 connector
  such as an M12 or XLR shell, is strongly preferred over relying on a label.
- If a jack for one of these runs is exposed on the outside of an enclosure, it
  reads as a network port to anyone who has not read this document. Recess it,
  key it, or use a connector that cannot mate with Ethernet.

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

**Landing on the Pi over the second Ethernet cable
([T568B](#cable-and-connector-conventions) colours):**

> **⚠️ This cable carries GPIO switch contacts, not Ethernet.** Never patch it
> into a switch, NIC, or PoE port — see
> [The booth runs are not Ethernet](#the-booth-runs-are-not-ethernet--never-patch-them-into-a-network).

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

> **Historical note (carbon on a USB dongle — superseded).** During bring-up the
> original carbon element passed usable, intelligible voice on a generic C-Media
> USB dongle's **plug-in bias alone** — no external bias circuit (option 4) —
> once the capture gain was pushed near the top of its range (~+17 dB) and the
> dongle's **Auto Gain Control was turned off** (AGC pumps the noise floor up
> on a quiet carbon source). It was still lo-fi and level varied as the granules
> packed. Those mixer levels are applied automatically at startup via the
> [`[audio.mixer]`](configuration.md#startup-alsa-mixer) config block so they
> survive reboots.
>
> **The booth no longer runs this way.** The handset path is now an electret
> element into a `MAX9814` preamp — see
> [As-built MAX9814 mic wiring](#as-built-max9814-mic-wiring). This note is kept
> because the carbon-direct result is what makes option 3 viable if you are
> reproducing the booth with a period element and no preamp.

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

> **⚠️ This cable carries DC and analog audio, not Ethernet.** Pins 1–2 hold the
> supply. Never patch it into a switch, NIC, or PoE port — see
> [The booth runs are not Ethernet](#the-booth-runs-are-not-ethernet--never-patch-them-into-a-network).

Five solid AWG leads land on the booth side: `green` ×2 (ground), `red` (5 V),
`blue` (**T** — receiver / audio *out*), and `black` (**TR** — transmitter /
audio *in*). Map them to the Ethernet conductors by
**[T568B](#cable-and-connector-conventions)** colour:

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
preamp sits **inside the booth**, powered over the Cat run's power pair, and its
output drives the `black` / **TR** conductor back to the Pi. The full transmitter
path is:

```text
electret element -> MAX9814 IN -> MAX9814 OUT -> black/TR conductor
  (green wire, RJ45 pin 6) -> Pi side -> USB dongle mic breakout tip
```

Note that **`TR` names two different things** in this document: the handset's
transmitter *terminal* (which lands on the mic element) and the `black`
*conductor* in the Cat run (which carries the preamp's output). The preamp sits
between them, which is why the
[Pi-side table](#landing-it-on-the-pi-side-usb-dongle-build) lands `TR` straight
on the dongle's mic tip — by that point the signal is already amplified.

The final, verified terminations are:

| Signal / terminal         | Wires to                                              |
| ------------------------- | ----------------------------------------------------- |
| **TR** (handset terminal) | Microphone (mic element)                              |
| Mic element               | `MAX9814` **mic input**                               |
| **OUT**                   | `black` / **TR** conductor of the Cat run             |
| **L** (loopback)          | Audio loopback into the `MAX9814` **mic inputs**      |
| **V+**                    | Cat run power pair → Pi header **3.3 V**              |
| **Pi GND** (star node)    | Both the **audio ground** *and* the **3.3 V ground**  |

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

## Power button and status LED

An optional **Adafruit 3350** illuminated pushbutton adds a physical
power/reset control plus an RGB status ring that mirrors the booth's state
machine. Both features are **opt-in and default-off**, so an existing booth is
completely unaffected until you wire the button and enable it in config.

The button is a momentary, normally-open (NO) switch combined with an RGB LED
ring. It is wired active-low against the Pi's internal pull-up: pressing it
pulls the switch pin low; each LED cathode is driven low to light that colour.

### Wiring (reference booth)

The reference booth (Raspberry Pi 4 Model B, Debian 13 trixie) lands the switch
on BCM 3 so the same button can wake the Pi after a safe power-off. The other
pins avoid the cooling fan (`18`) and existing phone harness (`17/22/27`):

| Button tab (Adafruit 3350) | Wire   | Terminal | BCM | Physical pin |
| -------------------------- | ------ | -------- | --- | ------------ |
| C+ (top-left)              | red    | 3V3      | —   | 17           |
| R  (top-right)             | yellow | IO5      | 5   | 29           |
| G  (bottom-right)          | green  | IO6      | 6   | 31           |
| B  (bottom-left)           | blue   | IO13     | 13  | 33           |
| switch (mid-right, large)  | white  | SCL      | 3   | 5            |
| switch (mid-left, large)   | black  | GND      | —   | 39           |

The LED ring shares a **common anode** on the 3V3 rail (button tab `C+`); the
three cathodes (`R`/`G`/`B`) are each driven low to light that colour.

> **⚠️ Colour codes mean different things on the two harnesses.** On the dial +
> hook harness, white/red/green/blue are hook/pulse/gate/common (see
> [As-built dial + hook wiring](#as-built-dial--hook-wiring-reference-booth)).
> On this button harness they are, per the table above: **red = LED anode
> (3V3)**, **yellow = red cathode**, **green = green cathode**, **blue = blue
> cathode**, **white = switch to BCM 3 (`SCL`)**, **black = switch to GND**.
> Note in
> particular that white is a *switch* lead here and red is *3V3* — wiring
> either by dial-harness habit shorts the LED supply. **Label both bundles** so
> they are never confused during install.

On the reference screw-terminal GPIO HAT, use the terminal labelled **`SCL`**.
Do not use **`SCLK`**: `SCL` is BCM 3 (I2C clock and the Pi 4 wake pin), while
`SCLK` is BCM 11 (SPI clock) and cannot wake a halted Pi. Connecting through a
HAT is equivalent to connecting directly to physical pin 5, provided the HAT
passes BCM 3 through and no I2C peripheral is using it.

### Only one colour at a time (shared current limit)

The Adafruit 3350 ring has a **single shared current-limiting resistor** across
all three cathodes — not one resistor per colour. This was verified on the
bench: if two or more cathodes are driven low at once, only the colour with the
lowest forward voltage lights (**red beats green, green beats blue**). Colour
mixing is therefore **physically impossible**, and time-multiplexing the
channels was tested and rejected as visibly unstable.

The firmware reflects this hardware fact: the [`booth_hal::LedColour`] type can
only ever be `Off`, `Red`, `Green`, or `Blue` — an unmixable colour is
unrepresentable — and the Pi adapter always drives at most one cathode low,
holding the other two high. See
[ADR 0009](adr/0009-status-led-power-button.md) for the full rationale.

### Colour and pattern per state

| Booth state                    | Colour | Pattern            |
| ------------------------------ | ------ | ------------------ |
| Booting / not ready            | blue   | slow pulse         |
| Idle (on hook)                 | green  | dim steady         |
| Dial tone / dialling           | green  | bright steady      |
| Playing a prompt               | blue   | steady             |
| Beep / recording               | red    | steady             |
| Finalising / uploading         | blue   | fast blink         |
| Error                          | red    | fast blink         |
| Shutting down                  | red    | fade to off        |

"Booting" and "shutting down" are transient runtime indications emitted
directly by the runtime; every other row is derived from the pure core's state
via `booth_core::status_led_for`.

### Button behaviour

- **Short press** (released before the hold threshold) → **reboot**
  (`systemctl reboot`).
- **Press and hold** past the threshold (default **3000 ms**,
  `power_button.hold_ms`) → **power off** (`systemctl poweroff`).
- **Press while halted** → **wake** the Pi. This is a Pi hardware function of
  BCM 3; the booth service is not running while the Pi is halted.

### Authorization for reboot / power off

`telephone-booth.service` runs as the unprivileged `phonebooth` user with an
empty `CapabilityBoundingSet` and `NoNewPrivileges=true`, so `systemctl reboot`
and `systemctl poweroff` are routed through logind and would otherwise fail the
default polkit check with *"Interactive authentication required"*.

The `.deb` therefore installs a narrowly scoped polkit rule at
`/usr/share/polkit-1/rules.d/50-telephone-booth-power.rules` granting **only**
the `phonebooth` user, and **only** the `org.freedesktop.login1.reboot` /
`power-off` actions (plus their `-multiple-sessions` variants). The
`*-ignore-inhibit` actions are deliberately **not** granted, so a running
inhibitor — an in-progress unattended upgrade, for example — still blocks the
shutdown. The package depends on `polkitd | policykit-1` so the rule is
actually evaluated.

If you run the binary outside the package, install that rule yourself (or run
the service as `root`), otherwise both button actions log
`poweroff failed: systemctl … exited with …` and nothing happens.

### Pi 4 wake behavior

On the Raspberry Pi 4, waking a **halted** Pi with a GPIO requires **BCM 3**
(I2C1 SCL). Wiring the booth button to BCM 3 does not change its runtime
behavior: the software can still distinguish a short press from a hold. It
adds wake-after-halt to the existing short-press reboot and long-hold
power-off actions.

BCM 3 is also the I2C1 clock line. Do not use this wiring if the booth needs
that I2C bus; keep the button on the default BCM 26 and use a separate wake
switch or an inline PSU controller instead.

See [`configuration.md`](configuration.md) for every config key and environment
override.
