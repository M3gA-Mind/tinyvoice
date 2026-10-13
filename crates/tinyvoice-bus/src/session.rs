//! Recording-session vocabulary: run phases, the pre-transcription gates, and
//! how a transcribed utterance is sequenced through the wake word.

use serde::{Deserialize, Serialize};

/// Running phase of a hotkey-driven recording session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    /// Not running.
    Stopped,
    /// Running and waiting for the next recording.
    Idle,
    /// Audio is being captured.
    Recording,
    /// A finished recording is being transcribed.
    Transcribing,
}

/// Thresholds a finished recording must clear before it is transcribed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecordingGates {
    /// Shorter recordings are discarded.
    pub min_duration_secs: f32,
    /// Recordings whose peak RMS is below this are treated as silence.
    pub silence_threshold: f32,
}

/// Outcome of applying [`RecordingGates`] to a finished recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    /// Transcribe the recording.
    Pass,
    /// Shorter than the minimum duration.
    TooShort,
    /// Peak energy below the silence threshold.
    Silent,
}

/// What to do with a transcribed always-on utterance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum UtteranceDisposition {
    /// Blank transcript or not addressed to the agent; do nothing.
    Ignored,
    /// Addressed with a command to route.
    Command {
        /// The command with the wake phrase removed.
        command: String,
    },
    /// Wake word spoken with nothing after it; acknowledge it.
    BareWake,
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
