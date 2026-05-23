//! Pure state machine for the Telephone Booth phone client.
//!
//! This crate is intentionally `no_std + alloc` and side-effect-free: it does
//! not perform any I/O, spawn any tasks, or take any wall-clock readings.
//! Every transition is computed by the single function
//! [`handle`](crate::handle) which takes `(State, Event)` and returns a new
//! `(State, Vec<Effect>)`. A runtime ([`booth-bin`](../booth_bin/index.html))
//! is responsible for translating effects into HAL calls and feeding the
//! resulting events back in.
//!
//! Keeping the core pure means:
//!
//! * Every legal transition is exhaustively unit-tested.
//! * The same code runs on a Pi (std + tokio adapters) and on an ESP32 / RP2040
//!   (alloc-only adapters) without a rewrite.
//! * Property tests with `proptest` can hammer the machine with random event
//!   sequences and assert invariants without booting hardware.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use booth_hal::{AudioRef, BoothStatus, BuiltinTone, QuestionId, RecordingId};
use serde::{Deserialize, Serialize};

/// Maximum number of rotary pulses we accept for a single digit. North
/// American rotaries pulse 1–10 times; anything above this is treated as
/// noise and the digit is rejected.
pub const MAX_PULSES_PER_DIGIT: u8 = 10;

/// Idle window before a sequence of pulses is closed and decoded into a
/// digit. The runtime supplies a `Tick` event when this elapses.
pub const PULSE_GROUP_TIMEOUT_MS: u64 = 350;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The state machine's discrete states. Every transition is the result of an
/// [`Event`] and produces a `Vec<Effect>` for the runtime to execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Receiver on hook. Nothing is playing or recording.
    Idle,
    /// Receiver off hook. Dial tone is playing; we are waiting for the user
    /// to dial a digit.
    DialTone,
    /// Receiver off hook. The user is rotating the rotary dial. `pulses`
    /// counts the pulses received so far in the current group.
    Dialing {
        /// Number of pulses received in this group so far.
        pulses: u8,
    },
    /// Playing the question audio (dial 1).
    PlayingQuestion {
        /// Operator's id for the question being played, so when the user's
        /// answer is uploaded we can associate it.
        question_id: QuestionId,
    },
    /// Short attention beep before recording starts.
    Beep {
        /// The question whose answer we are about to record.
        question_id: QuestionId,
    },
    /// Recording the caller's answer.
    Recording {
        /// The question being answered.
        question_id: QuestionId,
    },
    /// Uploading a finished recording to the operator.
    Uploading {
        /// Recording id from the audio adapter.
        recording_id: RecordingId,
        /// Question id this recording answers.
        question_id: QuestionId,
    },
    /// Playing a randomly chosen, previously-approved message (dial 2).
    PlayingMessage,
    /// Playing the instructions prompt (dial 3+).
    PlayingInstructions,
    /// A non-fatal error happened; the runtime should reset us back to
    /// `Idle` on the next `HookOn`. `reason` is short, human-readable.
    Error {
        /// Short human-readable reason.
        reason: String,
    },
}

impl State {
    /// Coarse status to advertise to the operator backend.
    #[must_use]
    pub fn status(&self) -> BoothStatus {
        match self {
            State::Idle => BoothStatus::Idle,
            State::DialTone | State::Dialing { .. } => BoothStatus::DialTone,
            State::PlayingQuestion { .. } | State::Beep { .. } => BoothStatus::PlayingQuestion,
            State::Recording { .. } => BoothStatus::Recording,
            State::Uploading { .. } => BoothStatus::Uploading,
            State::PlayingMessage => BoothStatus::PlayingMessage,
            State::PlayingInstructions => BoothStatus::PlayingInstructions,
            State::Error { .. } => BoothStatus::Idle,
        }
    }

    /// Short tag for logging / telemetry.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::DialTone => "dial_tone",
            State::Dialing { .. } => "dialing",
            State::PlayingQuestion { .. } => "playing_question",
            State::Beep { .. } => "beep",
            State::Recording { .. } => "recording",
            State::Uploading { .. } => "uploading",
            State::PlayingMessage => "playing_message",
            State::PlayingInstructions => "playing_instructions",
            State::Error { .. } => "error",
        }
    }
}

impl Default for State {
    fn default() -> Self {
        State::Idle
    }
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

/// Events the runtime feeds into the state machine. Each one is the result of
/// a HAL signal (GPIO edge, audio end-of-playback, upload finished, ...) or a
/// timer tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Hook switch closed: the receiver is back on the cradle.
    HookOn,
    /// Hook switch opened: the receiver was lifted.
    HookOff,
    /// One rotary pulse landed.
    RotaryPulse,
    /// The current pulse group timed out and decoded to `digit`.
    DigitDialed {
        /// 0..=9
        digit: u8,
    },
    /// The current playback finished naturally.
    PlaybackEnded,
    /// The recording timer ran out (max duration reached) or the user hung up.
    RecordingFinished {
        /// Id of the finished local recording.
        recording_id: RecordingId,
    },
    /// The upload completed successfully.
    UploadComplete,
    /// The upload failed (we'll log + return to dial tone).
    UploadFailed {
        /// Diagnostic message.
        reason: String,
    },
    /// Operator returned a question (for the runtime to start playing).
    QuestionReady {
        /// Question id from the operator.
        question_id: QuestionId,
    },
    /// Operator could not give us a question (no questions, transport error).
    QuestionFailed {
        /// Diagnostic message.
        reason: String,
    },
    /// Operator returned a message.
    MessageReady,
    /// Operator could not give us a message.
    MessageFailed {
        /// Diagnostic message.
        reason: String,
    },
    /// A periodic tick from the runtime, used (only) to time out pulse groups.
    Tick,
}

// ---------------------------------------------------------------------------
// Effect
// ---------------------------------------------------------------------------

/// Side effects requested by the state machine. The runtime executes each
/// effect against the appropriate HAL trait and feeds the resulting `Event`
/// back in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Start playing `source` on the audio sink.
    Play(AudioRef),
    /// Stop any audio that is playing.
    StopAudio,
    /// Begin recording the input device to a local FLAC file.
    StartRecording,
    /// Stop the current recording and return its id via `RecordingFinished`.
    StopRecording,
    /// Begin an upload for the finished `recording_id` answering `question_id`.
    UploadRecording {
        /// Recording to upload.
        recording_id: RecordingId,
        /// Question id we are answering.
        question_id: QuestionId,
    },
    /// Ask the operator for a random question.
    FetchRandomQuestion,
    /// Ask the operator for a random approved message.
    FetchRandomMessage,
    /// Push our current coarse status to the operator.
    PutStatus(BoothStatus),
    /// Reset the pulse-group timeout to fire `Tick` after
    /// [`PULSE_GROUP_TIMEOUT_MS`] of idle.
    ArmPulseTimeout,
    /// Cancel any in-flight pulse-group timeout.
    CancelPulseTimeout,
    /// Trace-level structured log entry (rendered into tracing by the
    /// runtime). Lets the pure core leave breadcrumbs in telemetry without
    /// taking a `tracing` dependency itself.
    Log {
        /// Short message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Transition function
// ---------------------------------------------------------------------------

/// Compute the next state and the effects to run for `(state, event)`.
///
/// The function is deterministic, allocation-light, and entirely independent
/// of wall-clock time — making it a perfect target for `proptest` and `insta`
/// snapshot tests.
#[must_use]
pub fn handle(state: State, event: Event) -> (State, Vec<Effect>) {
    use Event as E;
    use State as S;

    // Hanging up always returns us to Idle (with cleanup), regardless of the
    // current state. Tested as an invariant in `tests/transitions.rs`.
    if matches!(event, E::HookOn) {
        let mut effects = vec![
            Effect::StopAudio,
            Effect::CancelPulseTimeout,
            Effect::PutStatus(BoothStatus::Idle),
        ];
        if matches!(state, S::Recording { .. }) {
            effects.insert(0, Effect::StopRecording);
        }
        return (S::Idle, effects);
    }

    match (state, event) {
        // ---- Idle ----
        (S::Idle, E::HookOff) => (
            S::DialTone,
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::DialTone)),
                Effect::PutStatus(BoothStatus::DialTone),
            ],
        ),

        // ---- DialTone ----
        (S::DialTone, E::RotaryPulse) => (
            S::Dialing { pulses: 1 },
            vec![Effect::StopAudio, Effect::ArmPulseTimeout],
        ),

        // ---- Dialing ----
        (S::Dialing { pulses }, E::RotaryPulse) => {
            let next = pulses.saturating_add(1);
            if next > MAX_PULSES_PER_DIGIT {
                // Invalid digit; reset to dial tone and arm again on next pulse.
                (
                    S::DialTone,
                    vec![
                        Effect::Play(AudioRef::Builtin(BuiltinTone::DialTone)),
                        Effect::CancelPulseTimeout,
                        Effect::Log {
                            message: "rotary pulses exceeded maximum".to_string(),
                        },
                    ],
                )
            } else {
                (S::Dialing { pulses: next }, vec![Effect::ArmPulseTimeout])
            }
        }
        (S::Dialing { pulses }, E::Tick) => {
            // Close the pulse group, decode to a digit, and dispatch.
            let digit = if pulses == 10 { 0 } else { pulses };
            decode_digit(digit)
        }
        (S::Dialing { .. }, E::DigitDialed { digit }) => decode_digit(digit),

        // ---- PlayingQuestion -> Beep -> Recording ----
        (S::PlayingQuestion { question_id }, E::PlaybackEnded) => (
            S::Beep {
                question_id: question_id.clone(),
            },
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::Beep)),
                Effect::PutStatus(BoothStatus::PlayingQuestion),
            ],
        ),
        (S::Beep { question_id }, E::PlaybackEnded) => (
            S::Recording {
                question_id: question_id.clone(),
            },
            vec![
                Effect::StartRecording,
                Effect::PutStatus(BoothStatus::Recording),
            ],
        ),
        (S::Recording { question_id }, E::RecordingFinished { recording_id }) => (
            S::Uploading {
                recording_id: recording_id.clone(),
                question_id: question_id.clone(),
            },
            vec![
                Effect::UploadRecording {
                    recording_id,
                    question_id,
                },
                Effect::PutStatus(BoothStatus::Uploading),
            ],
        ),
        (S::Uploading { .. }, E::UploadComplete) => (
            S::DialTone,
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::DialTone)),
                Effect::PutStatus(BoothStatus::DialTone),
            ],
        ),
        (S::Uploading { .. }, E::UploadFailed { reason }) => (
            S::Error {
                reason: reason.clone(),
            },
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::LineBusy)),
                Effect::Log {
                    message: alloc::format!("upload failed: {reason}"),
                },
            ],
        ),

        // ---- Operator question / message lookups ----
        (S::DialTone, E::QuestionReady { question_id }) => (
            S::PlayingQuestion {
                question_id: question_id.clone(),
            },
            vec![
                Effect::Play(AudioRef::RemoteUrl(String::new())), // runtime fills in URL
                Effect::PutStatus(BoothStatus::PlayingQuestion),
                Effect::Log {
                    message: alloc::format!("question ready: {question_id}"),
                },
            ],
        ),
        (S::DialTone, E::QuestionFailed { reason }) => (
            S::Error {
                reason: reason.clone(),
            },
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::LineBusy)),
                Effect::Log {
                    message: alloc::format!("question fetch failed: {reason}"),
                },
            ],
        ),
        (S::DialTone, E::MessageReady) => (
            S::PlayingMessage,
            vec![
                Effect::Play(AudioRef::RemoteUrl(String::new())),
                Effect::PutStatus(BoothStatus::PlayingMessage),
            ],
        ),
        (S::DialTone, E::MessageFailed { reason }) => (
            S::Error {
                reason: reason.clone(),
            },
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::LineBusy)),
                Effect::Log {
                    message: alloc::format!("message fetch failed: {reason}"),
                },
            ],
        ),
        (S::PlayingMessage, E::PlaybackEnded) => (
            S::DialTone,
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::DialTone)),
                Effect::PutStatus(BoothStatus::DialTone),
            ],
        ),

        // ---- Instructions ----
        (S::PlayingInstructions, E::PlaybackEnded) => (
            S::DialTone,
            vec![
                Effect::Play(AudioRef::Builtin(BuiltinTone::DialTone)),
                Effect::PutStatus(BoothStatus::DialTone),
            ],
        ),

        // ---- Catch-all: anything not enumerated is a no-op ----
        (state, _) => (state, vec![]),
    }
}

/// Decode a digit (0..=9) into the appropriate transition out of `Dialing`.
fn decode_digit(digit: u8) -> (State, Vec<Effect>) {
    match digit {
        1 => (
            // Stay in DialTone until the operator hands us a question.
            State::DialTone,
            vec![Effect::FetchRandomQuestion, Effect::CancelPulseTimeout],
        ),
        2 => (
            State::DialTone,
            vec![Effect::FetchRandomMessage, Effect::CancelPulseTimeout],
        ),
        3..=9 | 0 => (
            State::PlayingInstructions,
            vec![
                Effect::Play(AudioRef::LocalFile("Instructions.flac".to_string())),
                Effect::CancelPulseTimeout,
                Effect::PutStatus(BoothStatus::PlayingInstructions),
            ],
        ),
        _ => (
            State::Error {
                reason: alloc::format!("invalid digit {digit}"),
            },
            vec![Effect::Play(AudioRef::Builtin(BuiltinTone::LineBusy))],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pickup_starts_dialtone() {
        let (next, effects) = handle(State::Idle, Event::HookOff);
        assert_eq!(next, State::DialTone);
        assert_eq!(
            effects[0],
            Effect::Play(AudioRef::Builtin(BuiltinTone::DialTone))
        );
    }

    #[test]
    fn hangup_from_anywhere_returns_to_idle() {
        for state in [
            State::DialTone,
            State::Dialing { pulses: 3 },
            State::PlayingMessage,
            State::Recording {
                question_id: "q1".into(),
            },
        ] {
            let (next, _) = handle(state, Event::HookOn);
            assert_eq!(next, State::Idle);
        }
    }

    #[test]
    fn three_pulses_then_tick_dials_three() {
        let mut s = State::DialTone;
        for _ in 0..3 {
            (s, _) = handle(s, Event::RotaryPulse);
        }
        assert_eq!(s, State::Dialing { pulses: 3 });
        let (s2, effects) = handle(s, Event::Tick);
        assert_eq!(s2, State::PlayingInstructions);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Play(AudioRef::LocalFile(_))))
        );
    }

    #[test]
    fn ten_pulses_decodes_to_zero_which_plays_instructions() {
        let mut s = State::DialTone;
        for _ in 0..10 {
            (s, _) = handle(s, Event::RotaryPulse);
        }
        let (s2, _) = handle(s, Event::Tick);
        assert_eq!(s2, State::PlayingInstructions);
    }

    #[test]
    fn dial_one_asks_operator_for_question() {
        let mut s = State::DialTone;
        (s, _) = handle(s, Event::RotaryPulse);
        let (_, effects) = handle(s, Event::Tick);
        assert!(effects.contains(&Effect::FetchRandomQuestion));
    }

    #[test]
    fn dial_two_asks_operator_for_message() {
        let mut s = State::DialTone;
        for _ in 0..2 {
            (s, _) = handle(s, Event::RotaryPulse);
        }
        let (_, effects) = handle(s, Event::Tick);
        assert!(effects.contains(&Effect::FetchRandomMessage));
    }
}
