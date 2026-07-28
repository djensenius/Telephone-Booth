# ADR 0003 — FLAC as recording format

**Status:** accepted.

## Context

The legacy installation recorded MP3s via the `microphone` Node module
piped through `lame`. Decisions to revisit:

- Lossy MP3 is unnecessary at booth scale (recordings are < 60 s each).
- We want broad decoder support on the operator side (browsers ship a
  Web Audio FLAC decoder via WASM at worst) and reliable seeking.
- We want a format with a real container so we can store sample rate,
  channel count, and a per-file `sha256` without a separate sidecar.

## Decision

Record and store **FLAC** (16-bit, 48 kHz, mono).

- Pi side encodes via `flacenc-rs` (no native deps).
- Browser side decodes via the existing Web Audio FLAC support or, as a
  fallback, `flac.wasm`.
- Files are content-addressed (`<sha256>.flac`) on Azure Blob Storage so
  duplicate uploads dedupe naturally.

## Consequences

**Good:**

- Lossless, no perceptible quality loss for archival.
- ~50% the size of WAV at our sample rate.
- One container format from microphone to operator UI.

**Trade-offs:**

- FLAC is bigger than MP3 by ~5×. At 60 s mono / 48 kHz / 16-bit that's
  ~5 MB per recording; budget Azure storage costs accordingly. Lifecycle
  rules in `docs/azure-storage.md` cover this.
- Encoding takes a few hundred ms of CPU on a Pi 4 per recording. Within
  budget.
- `flacenc` needs two structural fix-ups after encoding, applied in
  `crates/booth-pi/src/flac_frame.rs` and `finalize_recording`:
  - It writes sample-rate code `0` ("read it from `STREAMINFO`") in every
    frame header. That is spec-legal, but Apple CoreAudio decodes zero
    frames from such a stream, so recordings were silent in Finder,
    QuickTime, and the operator's browser player on macOS. We rewrite the
    field to the explicit code for the stream's rate and recompute the
    header CRC-8 and frame CRC-16. No audio is re-encoded.
  - It pads a trailing partial block out to a whole block, but computes
    `total_samples` and the `STREAMINFO` MD5 as if it had not, which makes
    `flac -t` reject the file. We pad the sample buffer to a whole number
    of blocks before encoding so the declared length matches what is
    actually stored.
