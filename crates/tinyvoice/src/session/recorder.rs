//! The hotkey -> record -> finish state machine.
//!
//! [`HotkeyRecorder`] decides what a hotkey fact, a finished recording setup or
//! a pipeline stage means for the recording session and returns the one
//! [`SessionAction`] the host must take. It owns no I/O, timer or thread: the
//! host starts native capture, runs the deferred-stop timer and the
//! transcription pipeline, and reports each outcome back as a [`SessionInput`].

use tinyvoice_bus::session::{
    DEFERRED_STOP_MS, SessionAction, SessionAdvance, SessionInput, SessionPhase, SessionStatus,
};

use super::RecordingSession;

/// A [`RecordingSession`] plus the setup / stop-intent bookkeeping that turns
/// hotkey events into start and stop actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HotkeyRecorder {
    session: RecordingSession,
    /// Native setup was requested and has not reported back.
    setup_pending: bool,
    /// A stop intent arrived while setup was pending.
    stop_pending: bool,
    /// Recording is live (setup finished, not yet stopped).
    live: bool,
}

impl HotkeyRecorder {
    /// A stopped recorder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            session: RecordingSession::new(),
            setup_pending: false,
            stop_pending: false,
            live: false,
        }
    }

    /// Current phase and generation.
    #[must_use]
    pub const fn status(&self) -> SessionStatus {
        SessionStatus {
            phase: self.session.phase(),
            generation: self.session.generation(),
        }
    }

    /// Apply `input` and return the verdict and the action the host must take.
    pub fn advance(&mut self, input: SessionInput) -> SessionAdvance {
        let (accepted, action) = self.apply(input);
        SessionAdvance {
            accepted,
            phase: self.session.phase(),
            generation: self.session.generation(),
            action,
        }
    }

    fn clear_flags(&mut self) {
        self.setup_pending = false;
        self.stop_pending = false;
        self.live = false;
    }

    fn stop_live(&mut self) -> (bool, SessionAction) {
        self.live = false;
        (
            true,
            SessionAction::StopRecording {
                generation: self.session.generation(),
            },
        )
    }

    fn apply(&mut self, input: SessionInput) -> (bool, SessionAction) {
        let running = self.session.phase() != SessionPhase::Stopped;
        match input {
            SessionInput::Run => (self.session.run(), SessionAction::None),
            SessionInput::Stop => {
                self.session.stop();
                self.clear_flags();
                (true, SessionAction::None)
            }
            _ if !running => (false, SessionAction::None),
            // Press toggles: stop a live recording, buffer a stop during
            // setup, otherwise start a new recording (even over a pipeline
            // still transcribing the previous one).
            SessionInput::Pressed => {
                if self.live {
                    self.stop_live()
                } else if self.setup_pending {
                    self.stop_pending = true;
                    (true, SessionAction::None)
                } else if let Some(generation) = self.session.restart_recording() {
                    self.setup_pending = true;
                    self.stop_pending = false;
                    (true, SessionAction::StartRecording { generation })
                } else {
                    (false, SessionAction::None)
                }
            }
            // Release only ever stops: with no recording it is a no-op
            // (normal for keys whose release is unreliable).
            SessionInput::Released => {
                if self.live {
                    self.stop_live()
                } else if self.setup_pending {
                    self.stop_pending = true;
                    (true, SessionAction::None)
                } else {
                    (true, SessionAction::None)
                }
            }
            SessionInput::SetupReady { stop_buffered } => {
                if !self.setup_pending {
                    return (false, SessionAction::None);
                }
                self.setup_pending = false;
                self.live = true;
                let generation = self.session.generation();
                if self.stop_pending || stop_buffered {
                    self.stop_pending = false;
                    (
                        true,
                        SessionAction::DeferStop {
                            generation,
                            after_ms: DEFERRED_STOP_MS,
                        },
                    )
                } else {
                    (true, SessionAction::RecordingLive { generation })
                }
            }
            SessionInput::SetupFailed => {
                if !self.setup_pending {
                    return (false, SessionAction::None);
                }
                self.clear_flags();
                let generation = self.session.generation();
                (self.session.finish(generation), SessionAction::None)
            }
            SessionInput::DeferredStopElapsed => {
                if self.live {
                    self.stop_live()
                } else {
                    (false, SessionAction::None)
                }
            }
            SessionInput::Transcribing { generation } => {
                (self.session.begin_transcribing(generation), SessionAction::None)
            }
            SessionInput::Finished { generation } => {
                (self.session.finish(generation), SessionAction::None)
            }
        }
    }
}

#[cfg(test)]
#[path = "recorder_tests.rs"]
mod tests;
