//! The recording session: a generation-guarded run state, the gates a finished
//! recording must clear, wake-word / fast-path sequencing of a transcript, and
//! a driver that sequences a recording through host-supplied *transcribe* and
//! *deliver* callbacks.
//!
//! Everything here is synchronous and I/O-free. The host owns the microphone,
//! the STT call, and text delivery; it plugs them in through [`SessionHost`].

use crate::intent::{self, VoiceIntent};
use crate::transcript::{self, Mode};
pub use tinyvoice_bus::session::{GateVerdict, RecordingGates, SessionPhase, UtteranceDisposition};

mod recorder;
pub use recorder::HotkeyRecorder;

/// A run state whose updates are tied to the recording that requested them.
///
/// A slow transcription finishing after the user has already started the next
/// recording must not flip the state back to idle, so every transition after
/// the first carries the generation it belongs to and is dropped if stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingSession {
    phase: SessionPhase,
    generation: u64,
}

impl Default for RecordingSession {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingSession {
    /// A stopped session at generation zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            phase: SessionPhase::Stopped,
            generation: 0,
        }
    }

    /// Current phase.
    #[must_use]
    pub const fn phase(&self) -> SessionPhase {
        self.phase
    }

    /// Generation of the latest recording.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// `Stopped` -> `Idle`. Returns `false` (and changes nothing) if already running.
    pub fn run(&mut self) -> bool {
        if self.phase != SessionPhase::Stopped {
            return false;
        }
        self.phase = SessionPhase::Idle;
        true
    }

    /// Any phase -> `Stopped`; invalidates every in-flight generation.
    pub fn stop(&mut self) {
        self.generation += 1;
        self.phase = SessionPhase::Stopped;
    }

    /// Begin a recording. Only valid when `Idle`; returns the new generation.
    pub fn begin_recording(&mut self) -> Option<u64> {
        if self.phase != SessionPhase::Idle {
            return None;
        }
        self.generation += 1;
        self.phase = SessionPhase::Recording;
        Some(self.generation)
    }

    /// Begin a recording from `Idle` **or** `Transcribing`; returns the new generation.
    ///
    /// A hotkey press while the previous recording is still being transcribed
    /// must start the next one, so rapid consecutive presses are never missed.
    /// The new generation makes the older pipeline's updates stale. Refused
    /// when `Stopped` or already `Recording`.
    pub fn restart_recording(&mut self) -> Option<u64> {
        if matches!(self.phase, SessionPhase::Stopped | SessionPhase::Recording) {
            return None;
        }
        self.generation += 1;
        self.phase = SessionPhase::Recording;
        Some(self.generation)
    }

    /// `Recording` -> `Transcribing` for `generation`. Stale generations are dropped.
    pub fn begin_transcribing(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.phase != SessionPhase::Recording {
            return false;
        }
        self.phase = SessionPhase::Transcribing;
        true
    }

    /// Return to `Idle` for `generation`. Stale generations are dropped.
    pub fn finish(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.phase == SessionPhase::Stopped {
            return false;
        }
        self.phase = SessionPhase::Idle;
        true
    }
}

/// Apply the minimum-duration and silence gates to a finished recording.
#[must_use]
pub fn gate_recording(gates: RecordingGates, duration_secs: f32, peak_rms: f32) -> GateVerdict {
    if duration_secs < gates.min_duration_secs {
        GateVerdict::TooShort
    } else if peak_rms < gates.silence_threshold {
        GateVerdict::Silent
    } else {
        GateVerdict::Pass
    }
}

/// Sequence a transcript through the wake-word gate.
#[must_use]
pub fn sequence_utterance(transcript: &str, wake_word: &str) -> UtteranceDisposition {
    let text = transcript.trim();
    if text.is_empty() {
        return UtteranceDisposition::Ignored;
    }
    if let Some(command) = intent::extract_command(text, wake_word) {
        return UtteranceDisposition::Command { command };
    }
    if intent::wake_word_present(text, wake_word) {
        UtteranceDisposition::BareWake
    } else {
        UtteranceDisposition::Ignored
    }
}

/// What the host should do with a routed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Execute a recognised intent locally; on failure fall back to the agent.
    FastPath {
        /// The recognised intent.
        intent: VoiceIntent,
        /// The original command, used for the agent fallback.
        command: String,
    },
    /// Hand the command to the agent.
    Agent {
        /// The command text.
        command: String,
    },
}

/// Route a command: a recognised intent runs locally, anything else goes to the agent.
#[must_use]
pub fn plan_delivery(command: &str) -> Delivery {
    match intent::route(command) {
        VoiceIntent::Unknown => Delivery::Agent {
            command: command.to_string(),
        },
        intent => Delivery::FastPath {
            intent,
            command: command.to_string(),
        },
    }
}

/// The text a bare wake word is acknowledged with.
pub const BARE_WAKE_ACK: &str = "hello";

/// Host callbacks the session driver needs.
pub trait SessionHost {
    /// Transcribe a finished recording's audio.
    ///
    /// # Errors
    /// The host's transcription failure, as display text.
    fn transcribe(&mut self, audio: &[u8]) -> Result<String, String>;
    /// Deliver a planned command (run the fast path or hand to the agent).
    ///
    /// # Errors
    /// The host's delivery failure; for a fast path the driver falls back to the agent.
    fn deliver(&mut self, delivery: Delivery) -> Result<(), String>;
}

/// How a processed recording ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Dropped by a gate before transcription.
    Gated(GateVerdict),
    /// The transcript was blank, a hallucination, or not addressed to the agent.
    Dropped,
    /// Transcription failed.
    TranscribeFailed(String),
    /// The command (or acknowledgement) was delivered.
    Delivered,
    /// Delivery failed even after the agent fallback.
    DeliverFailed(String),
}

/// Process one always-on recording: gate, transcribe, wake-word sequence,
/// route, deliver. Routing can only shortcut: a failed fast path is retried
/// through the agent, and an unaddressed utterance is never delivered.
pub fn process_utterance<H: SessionHost>(
    host: &mut H,
    gates: RecordingGates,
    wake_word: &str,
    audio: &[u8],
    duration_secs: f32,
    peak_rms: f32,
) -> Outcome {
    let verdict = gate_recording(gates, duration_secs, peak_rms);
    if verdict != GateVerdict::Pass {
        return Outcome::Gated(verdict);
    }
    let text = match host.transcribe(audio) {
        Ok(text) => text,
        Err(error) => return Outcome::TranscribeFailed(error),
    };
    if transcript::is_hallucinated(&text, Mode::Conversation) {
        return Outcome::Dropped;
    }
    let command = match sequence_utterance(&text, wake_word) {
        UtteranceDisposition::Ignored => return Outcome::Dropped,
        UtteranceDisposition::Command { command } => command,
        UtteranceDisposition::BareWake => BARE_WAKE_ACK.to_string(),
    };
    let delivery = plan_delivery(&command);
    let fast = matches!(delivery, Delivery::FastPath { .. });
    match host.deliver(delivery) {
        Ok(()) => Outcome::Delivered,
        Err(_) if fast => match host.deliver(Delivery::Agent { command }) {
            Ok(()) => Outcome::Delivered,
            Err(error) => Outcome::DeliverFailed(error),
        },
        Err(error) => Outcome::DeliverFailed(error),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
