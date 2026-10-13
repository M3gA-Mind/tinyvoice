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

/// An input to the hotkey-driven recording state machine.
///
/// The host reports what happened (a key fact, a finished setup, a pipeline
/// stage); the machine answers with the [`SessionAction`] it must take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum SessionInput {
    /// `Stopped` -> `Idle`. Rejected if already running.
    Run,
    /// Any phase -> `Stopped`; invalidates every in-flight generation.
    Stop,
    /// The hotkey was pressed.
    Pressed,
    /// The hotkey was released.
    Released,
    /// Native recording setup requested by [`SessionAction::StartRecording`] finished.
    SetupReady {
        /// The host found a hotkey event queued behind the setup result, which
        /// counts as a stop intent (it lost a `select!` race against the setup).
        stop_buffered: bool,
    },
    /// Native recording setup failed or its task was dropped.
    SetupFailed,
    /// The deferred-stop delay requested by [`SessionAction::DeferStop`] elapsed.
    DeferredStopElapsed,
    /// The pipeline for `generation` began transcribing.
    Transcribing {
        /// Generation the pipeline was started for.
        generation: u64,
    },
    /// The pipeline for `generation` finished.
    Finished {
        /// Generation the pipeline was started for.
        generation: u64,
    },
}

/// What the host must do after a [`SessionInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SessionAction {
    /// Nothing; the input was buffered, redundant or stale.
    None,
    /// Start native recording off the event loop, then report `setup_ready`
    /// or `setup_failed`. Cancels any deferred stop.
    StartRecording {
        /// Generation of the new recording.
        generation: u64,
    },
    /// Setup finished with no stop intent; the recording is live.
    RecordingLive {
        /// Generation of the live recording.
        generation: u64,
    },
    /// A stop intent arrived during setup. Keep recording for `after_ms`, then
    /// report `deferred_stop_elapsed` so the capture holds actual speech.
    DeferStop {
        /// Generation of the live recording.
        generation: u64,
        /// Minimum extra recording time in milliseconds.
        after_ms: u64,
    },
    /// Stop the live recording and hand it to the pipeline. Cancels any
    /// deferred stop.
    StopRecording {
        /// Generation of the recording being stopped.
        generation: u64,
    },
}

/// Reply to a [`SessionInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAdvance {
    /// Whether the input applied. `false` for a stale generation, a second
    /// `run`, or any input to a stopped session.
    pub accepted: bool,
    /// Phase after the input.
    pub phase: SessionPhase,
    /// Generation of the latest recording after the input.
    pub generation: u64,
    /// What the host must do next.
    pub action: SessionAction,
}

/// Point-in-time snapshot of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatus {
    /// Current phase.
    pub phase: SessionPhase,
    /// Generation of the latest recording.
    pub generation: u64,
}

/// Extra recording time after a stop intent that arrived during setup.
pub const DEFERRED_STOP_MS: u64 = 1500;

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
