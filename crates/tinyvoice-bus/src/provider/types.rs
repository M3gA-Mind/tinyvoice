//! Request, response and session values for host-supplied cloud voice.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Encoded audio and caller-selected transcription hints.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptionRequest {
    /// Base64-encoded audio bytes.
    pub audio_base64: String,
    /// Optional transcription model.
    pub model: Option<String>,
    /// Optional language hint.
    pub language: Option<String>,
    /// Optional input MIME type.
    pub mime_type: Option<String>,
    /// Optional original filename.
    pub file_name: Option<String>,
}

/// The transcription text returned by a provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transcription {
    /// Recognized speech.
    pub text: String,
}

/// Reply text and caller-selected speech hints.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SynthesisRequest {
    /// Text to speak.
    pub text: String,
    /// Optional voice selection.
    pub voice_id: Option<String>,
    /// Optional synthesis model.
    pub model_id: Option<String>,
    /// Optional output encoding.
    pub output_format: Option<String>,
    /// Provider-specific voice settings, forwarded without interpretation.
    pub voice_settings: Option<Value>,
}

/// The hosted session the caller wants to bootstrap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeRequest {
    /// A signed conversational voice-agent session.
    VoiceAgent,
    /// A Gemini relay ticket with provider-owned live session options.
    GeminiLive {
        /// Voice/model/session options; no backend credentials or base URL.
        options: Value,
    },
}

/// Short-lived connection facts returned by the backend provider.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeSession {
    /// A signed conversational voice-agent session.
    VoiceAgent {
        /// Short-lived signed websocket URL.
        signed_url: String,
        /// Provider agent identifier.
        agent_id: String,
        /// Short-lived user binding returned by the backend.
        user_token: String,
    },
    /// A Gemini relay session.
    GeminiLive {
        /// Signed relay websocket URL.
        ws_url: String,
        /// Backend session identifier.
        session_id: String,
        /// Selected live model.
        model: String,
    },
}

impl fmt::Debug for RealtimeSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VoiceAgent { agent_id, .. } => formatter
                .debug_struct("VoiceAgent")
                .field("signed_url", &"[REDACTED]")
                .field("agent_id", agent_id)
                .field("user_token", &"[REDACTED]")
                .finish(),
            Self::GeminiLive {
                session_id, model, ..
            } => formatter
                .debug_struct("GeminiLive")
                .field("ws_url", &"[REDACTED]")
                .field("session_id", session_id)
                .field("model", model)
                .finish(),
        }
    }
}
