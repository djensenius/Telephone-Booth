//! Structural post-processing of a `flacenc`-encoded FLAC stream.
//!
//! `flacenc` always emits sample-rate code `0` ("read it from `STREAMINFO`")
//! in every frame header. That is legal per the FLAC specification, but Apple
//! CoreAudio (Finder, QuickTime, `afplay`, `afconvert`) does not implement it:
//! it parses the header, reports the correct duration, and then decodes zero
//! frames. The Telephone-Booth-Operator web player inherits the same failure
//! on macOS.
//!
//! [`rewrite_frame_sample_rates`] walks the encoded byte stream and replaces
//! that field with the explicit code for the stream's sample rate, then
//! recomputes the header CRC-8 and the frame CRC-16. The chosen codes are all
//! fixed-width, so no byte is inserted or removed: frame lengths, `STREAMINFO`
//! frame-size bounds, and the audio payload are untouched. Nothing is
//! re-encoded and no quality is lost.

/// FLAC frame-header CRC-8 polynomial (`x^8 + x^2 + x + 1`).
const CRC8_POLY: u8 = 0x07;
/// FLAC frame CRC-16 polynomial (`x^16 + x^15 + x^2 + 1`).
const CRC16_POLY: u16 = 0x8005;

/// Bytes preceding the UTF-8 coded frame/sample number in a frame header.
const FRAME_HEADER_PREFIX_LEN: usize = 4;
/// Size of the trailing CRC-16 footer on every frame.
const FRAME_FOOTER_LEN: usize = 2;

const CRC8_TABLE: [u8; 256] = build_crc8_table();
const CRC16_TABLE: [u16; 256] = build_crc16_table();

const fn build_crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ CRC8_POLY
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn build_crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ CRC16_POLY
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

fn crc8(bytes: &[u8]) -> u8 {
    bytes
        .iter()
        .fold(0u8, |crc, byte| CRC8_TABLE[usize::from(crc ^ byte)])
}

fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0u16, |crc, byte| {
        (crc << 8) ^ CRC16_TABLE[usize::from((crc >> 8) as u8 ^ byte)]
    })
}

/// Maps a sample rate to the FLAC frame-header sample-rate code that encodes
/// it inline without any trailing rate bytes.
///
/// Returns `None` for rates outside the fixed table. The variable-width codes
/// (`12`..=`14`) are deliberately not used: they would lengthen every frame
/// header and invalidate the `STREAMINFO` frame-size bounds, which is more
/// surgery than this pass is meant to do.
const fn sample_rate_code(rate_hz: u32) -> Option<u8> {
    Some(match rate_hz {
        88_200 => 1,
        176_400 => 2,
        192_000 => 3,
        8_000 => 4,
        16_000 => 5,
        22_050 => 6,
        24_000 => 7,
        32_000 => 8,
        44_100 => 9,
        48_000 => 10,
        96_000 => 11,
        _ => return None,
    })
}

/// Byte length of a UTF-8 coded number given its leading byte.
const fn utf8_coded_number_len(lead: u8) -> Option<usize> {
    Some(match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        0xF8..=0xFB => 5,
        0xFC..=0xFD => 6,
        0xFE => 7,
        _ => return None,
    })
}

/// Number of bytes a frame header occupies, including its CRC-8 byte.
fn frame_header_len(frame: &[u8]) -> Option<usize> {
    // 14-bit sync code `0b11111111111110`, then a reserved 0 bit and the
    // blocking-strategy bit.
    if frame.len() < FRAME_HEADER_PREFIX_LEN + 2 || frame[0] != 0xFF || frame[1] & 0xFC != 0xF8 {
        return None;
    }
    let coded_number_len = utf8_coded_number_len(frame[FRAME_HEADER_PREFIX_LEN])?;
    // Block-size codes 6 and 7 append an 8-bit / 16-bit block size.
    let block_size_len = match frame[2] >> 4 {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    // Sample-rate codes 12..=14 append an 8-bit / 16-bit rate.
    let sample_rate_len = match frame[2] & 0x0F {
        12 => 1,
        13 | 14 => 2,
        15 => return None,
        _ => 0,
    };
    Some(FRAME_HEADER_PREFIX_LEN + coded_number_len + block_size_len + sample_rate_len + 1)
}

/// Checks that every frame boundary implied by `frame_lengths` really is one.
///
/// A frame must start with the sync code, parse as a header, and carry a
/// CRC-8 that matches its own bytes. Requiring this for *all* frames before
/// touching any of them means a bad offset makes the pass a no-op instead of
/// letting it write garbage into the middle of the audio payload.
fn frames_are_intact(encoded: &[u8], first_frame: usize, frame_lengths: &[usize]) -> bool {
    let mut offset = first_frame;
    for &len in frame_lengths {
        let Some(frame) = encoded.get(offset..offset + len) else {
            return false;
        };
        let Some(header_len) = frame_header_len(frame) else {
            return false;
        };
        if frame.len() < header_len + FRAME_FOOTER_LEN
            || frame[header_len - 1] != crc8(&frame[..header_len - 1])
        {
            return false;
        }
        offset += len;
    }
    true
}

/// Rewrites one frame in place. Returns `false` if the frame does not look
/// like a `flacenc`-produced frame with an unspecified sample rate.
fn rewrite_frame(frame: &mut [u8], code: u8) -> bool {
    let Some(header_len) = frame_header_len(frame) else {
        return false;
    };
    // Already explicit (or variable-width): leave it alone.
    if frame[2] & 0x0F != 0 || frame.len() < header_len + FRAME_FOOTER_LEN {
        return false;
    }
    frame[2] = (frame[2] & 0xF0) | code;
    let crc8_index = header_len - 1;
    frame[crc8_index] = crc8(&frame[..crc8_index]);

    let footer_index = frame.len() - FRAME_FOOTER_LEN;
    let checksum = crc16(&frame[..footer_index]).to_be_bytes();
    frame[footer_index..].copy_from_slice(&checksum);
    true
}

/// Rewrites the sample-rate field of every frame header in `encoded`.
///
/// `frame_lengths` must list the serialized byte length of each frame in
/// order; `flacenc` exposes these as `Frame::count_bits() >> 3`. Their sum is
/// used to locate the first frame, so the metadata blocks never need parsing.
///
/// The stream is validated in full before a single byte is written, so this
/// either rewrites every frame or leaves `encoded` untouched.
///
/// Returns the number of frames rewritten. `0` means the stream was left
/// byte-for-byte unchanged — either the rate has no fixed-width code, the
/// lengths did not add up, or the frames were already explicit.
pub fn rewrite_frame_sample_rates(
    encoded: &mut [u8],
    frame_lengths: &[usize],
    rate_hz: u32,
) -> usize {
    let Some(code) = sample_rate_code(rate_hz) else {
        tracing::warn!(
            rate_hz,
            "no fixed-width FLAC sample-rate code; frame headers left unspecified, \
             which Apple CoreAudio cannot decode"
        );
        return 0;
    };
    let total: usize = frame_lengths.iter().sum();
    let Some(first_frame) = encoded.len().checked_sub(total) else {
        tracing::warn!("FLAC frame lengths exceed the encoded stream; skipping header rewrite");
        return 0;
    };
    if !encoded.starts_with(b"fLaC") {
        tracing::warn!("encoded FLAC stream is missing its magic number; skipping header rewrite");
        return 0;
    }
    if !frames_are_intact(encoded, first_frame, frame_lengths) {
        tracing::warn!(
            "encoded FLAC frame boundaries did not validate; skipping header rewrite. \
             Recordings will not play back under Apple CoreAudio"
        );
        return 0;
    }

    let mut offset = first_frame;
    let mut rewritten = 0;
    for &len in frame_lengths {
        let Some(frame) = encoded.get_mut(offset..offset + len) else {
            break;
        };
        if rewrite_frame(frame, code) {
            rewritten += 1;
        }
        offset += len;
    }
    if rewritten != frame_lengths.len() {
        tracing::warn!(
            rewritten,
            total_frames = frame_lengths.len(),
            "only some FLAC frame headers could be rewritten"
        );
    }
    rewritten
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn crc8_matches_reference_vectors() {
        // "123456789" under CRC-8/SMBUS (init 0, poly 0x07) is 0xF4.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc8(&[]), 0x00);
    }

    #[test]
    fn crc16_matches_reference_vectors() {
        // "123456789" under CRC-16/UMTS (init 0, poly 0x8005) is 0xFEE8.
        assert_eq!(crc16(b"123456789"), 0xFEE8);
        assert_eq!(crc16(&[]), 0x0000);
    }

    #[test]
    fn sample_rate_codes_cover_the_fixed_table() {
        assert_eq!(sample_rate_code(48_000), Some(10));
        assert_eq!(sample_rate_code(44_100), Some(9));
        assert_eq!(sample_rate_code(8_000), Some(4));
        // 11.025 kHz has no fixed-width code.
        assert_eq!(sample_rate_code(11_025), None);
    }

    #[test]
    fn utf8_coded_number_len_rejects_continuation_bytes() {
        assert_eq!(utf8_coded_number_len(0x00), Some(1));
        assert_eq!(utf8_coded_number_len(0xC0), Some(2));
        assert_eq!(utf8_coded_number_len(0xFE), Some(7));
        assert_eq!(utf8_coded_number_len(0x80), None);
        assert_eq!(utf8_coded_number_len(0xFF), None);
    }

    /// Builds a minimal but structurally valid frame: header with sample-rate
    /// code 0, one payload byte, and a CRC-16 footer.
    fn synthetic_frame() -> Vec<u8> {
        let mut frame = vec![0xFF, 0xF8, 0xC0, 0x08, 0x00, 0x00, 0xAB, 0x00, 0x00];
        let header_len = frame_header_len(&frame).expect("header parses");
        frame[header_len - 1] = crc8(&frame[..header_len - 1]);
        let footer = frame.len() - FRAME_FOOTER_LEN;
        let checksum = crc16(&frame[..footer]).to_be_bytes();
        frame[footer..].copy_from_slice(&checksum);
        frame
    }

    #[test]
    fn rewrite_frame_sets_code_and_fixes_both_crcs() {
        let mut frame = synthetic_frame();
        assert!(rewrite_frame(&mut frame, 10));

        assert_eq!(frame[2] & 0x0F, 10, "sample-rate code was not written");
        let header_len = frame_header_len(&frame).expect("header still parses");
        assert_eq!(
            frame[header_len - 1],
            crc8(&frame[..header_len - 1]),
            "header CRC-8 was not recomputed"
        );
        let footer = frame.len() - FRAME_FOOTER_LEN;
        assert_eq!(
            frame[footer..],
            crc16(&frame[..footer]).to_be_bytes(),
            "frame CRC-16 was not recomputed"
        );
    }

    #[test]
    fn rewrite_frame_skips_already_explicit_frames() {
        let mut frame = synthetic_frame();
        assert!(rewrite_frame(&mut frame, 10));
        let once = frame.clone();
        assert!(!rewrite_frame(&mut frame, 10));
        assert_eq!(frame, once, "a second pass must be a no-op");
    }

    #[test]
    fn rewrite_stream_leaves_unknown_rates_untouched() {
        let frame = synthetic_frame();
        let mut encoded = b"fLaC".to_vec();
        encoded.extend_from_slice(&frame);
        let original = encoded.clone();
        assert_eq!(
            rewrite_frame_sample_rates(&mut encoded, &[frame.len()], 11_025),
            0
        );
        assert_eq!(encoded, original);
    }

    #[test]
    fn rewrite_stream_rejects_inconsistent_frame_lengths() {
        let frame = synthetic_frame();
        let mut encoded = b"fLaC".to_vec();
        encoded.extend_from_slice(&frame);
        let original = encoded.clone();
        let bogus_len = encoded.len() + 1;
        assert_eq!(
            rewrite_frame_sample_rates(&mut encoded, &[bogus_len], 48_000),
            0
        );
        assert_eq!(encoded, original);
    }

    /// Encodes a signal exactly the way `finalize_recording` does and returns
    /// the serialized stream together with each frame's byte length.
    fn encode(samples: &[i32], channels: usize, rate_hz: u32) -> (Vec<u8>, Vec<usize>) {
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;

        let encoder = flacenc::config::Encoder::default()
            .into_verified()
            .expect("default encoder config verifies");
        let block = encoder.block_size * channels;
        let mut padded = samples.to_vec();
        padded.resize(padded.len().div_ceil(block) * block, 0);
        let source =
            flacenc::source::MemSource::from_samples(&padded, channels, 16, rate_hz as usize);
        let stream = flacenc::encode_with_fixed_block_size(&encoder, source, encoder.block_size)
            .expect("encode succeeds");
        let lengths: Vec<usize> = (0..stream.frame_count())
            .filter_map(|index| stream.frame(index))
            .map(|frame| frame.count_bits() >> 3)
            .collect();
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).expect("serialize succeeds");
        (sink.into_inner(), lengths)
    }

    /// Walks the frames of `encoded` and returns, per frame, the sample-rate
    /// code plus whether both CRCs check out.
    fn inspect_frames(encoded: &[u8], lengths: &[usize]) -> Vec<(u8, bool)> {
        let total: usize = lengths.iter().sum();
        let mut offset = encoded.len() - total;
        let mut out = Vec::with_capacity(lengths.len());
        for &len in lengths {
            let frame = &encoded[offset..offset + len];
            let header_len = frame_header_len(frame).expect("frame header parses");
            let header_ok = frame[header_len - 1] == crc8(&frame[..header_len - 1]);
            let footer = len - FRAME_FOOTER_LEN;
            let frame_ok = frame[footer..] == crc16(&frame[..footer]).to_be_bytes();
            out.push((frame[2] & 0x0F, header_ok && frame_ok));
            offset += len;
        }
        assert_eq!(offset, encoded.len(), "frame lengths must cover the stream");
        out
    }

    #[test]
    fn misaligned_frame_lengths_leave_the_stream_untouched() {
        let samples: Vec<i32> = (0..12_000).map(|i: i32| (i % 97) - 48).collect();
        let (encoded, lengths) = encode(&samples, 1, 48_000);
        let mut shifted = encoded.clone();
        // Claim the first frame is one byte shorter than it is: every later
        // boundary now lands inside the audio payload.
        let mut bad = lengths.clone();
        bad[0] -= 1;
        assert_eq!(rewrite_frame_sample_rates(&mut shifted, &bad, 48_000), 0);
        assert_eq!(shifted, encoded, "a bad offset must not corrupt the stream");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)] // Test signal is bounded well inside i32.
    fn stereo_recordings_are_rewritten_and_still_decode() {
        let samples: Vec<i32> = (0..60_000)
            .map(|i| ((f64::from(i) / 13.0).sin() * 7000.0) as i32)
            .collect();
        let (mut encoded, lengths) = encode(&samples, 2, 44_100);
        assert_eq!(
            rewrite_frame_sample_rates(&mut encoded, &lengths, 44_100),
            lengths.len()
        );
        for (index, (code, crcs_ok)) in inspect_frames(&encoded, &lengths).into_iter().enumerate() {
            assert_eq!(code, 9, "frame {index} does not declare 44.1 kHz");
            assert!(crcs_ok, "frame {index} has a stale CRC");
        }
        let mut reader = claxon::FlacReader::new(std::io::Cursor::new(encoded))
            .expect("patched stereo stream parses");
        assert_eq!(reader.streaminfo().channels, 2);
        let decoded: Vec<i32> = reader
            .samples()
            .map(|s| s.expect("sample decodes"))
            .collect();
        assert_eq!(&decoded[..samples.len()], &samples[..]);
    }

    /// The regression guard for issue #127: every frame header must name its
    /// sample rate explicitly, because Apple CoreAudio decodes zero frames
    /// when the field says "read it from `STREAMINFO`".
    #[test]
    #[allow(clippy::cast_possible_truncation)] // Test signal is bounded well inside i32.
    fn encoded_recording_declares_sample_rate_in_every_frame() {
        let samples: Vec<i32> = (0..40_000)
            .map(|i| ((f64::from(i) / 20.0).sin() * 8000.0) as i32)
            .collect();
        let (mut encoded, lengths) = encode(&samples, 1, 48_000);
        assert!(lengths.len() > 1, "expected a multi-frame stream");

        // Precondition: this is the bug as `flacenc` ships it.
        assert!(
            inspect_frames(&encoded, &lengths)
                .iter()
                .all(|&(code, ok)| code == 0 && ok),
            "flacenc no longer writes sample-rate code 0; revisit this pass"
        );

        let rewritten = rewrite_frame_sample_rates(&mut encoded, &lengths, 48_000);
        assert_eq!(rewritten, lengths.len());

        for (index, (code, crcs_ok)) in inspect_frames(&encoded, &lengths).into_iter().enumerate() {
            assert_eq!(code, 10, "frame {index} does not declare 48 kHz");
            assert!(crcs_ok, "frame {index} has a stale CRC");
        }

        // The audio must survive the rewrite untouched.
        let mut reader =
            claxon::FlacReader::new(std::io::Cursor::new(encoded)).expect("patched stream parses");
        assert_eq!(reader.streaminfo().sample_rate, 48_000);
        let decoded: Vec<i32> = reader
            .samples()
            .map(|s| s.expect("sample decodes"))
            .collect();
        assert_eq!(&decoded[..samples.len()], &samples[..]);
    }
}
