# Built-in audio tones

`dial-tone.flac`, `beep.flac`, and `line-busy.flac` are license-free generated assets. They were synthesized from sine waves in this repository (350 Hz + 440 Hz for the dial tone, 1 kHz for the beep, 480 Hz + 620 Hz gated at 0.5 s on / 0.5 s off for the line-busy signal) and encoded to FLAC once so normal Cargo builds do not depend on an external `flac` binary.

`call-unavailable.flac` is a recorded "your call cannot be completed as dialed" voice prompt (`BuiltinTone::CallUnavailable`), played when the caller dials a digit with no assigned action (3-9). It is bundled as 48 kHz mono 16-bit FLAC to match the other embedded tones.
