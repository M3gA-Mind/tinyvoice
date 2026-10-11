//! Host-supplied cloud voice transport, independent of native module behavior.
//!
//! A host implements this interface in its backend layer and installs it through
//! its own composition seam. Requests contain voice inputs, never backend URLs,
//! credentials, or product configuration. The implementation owns authentication,
//! endpoint selection and network I/O. Capture, VAD, hotkeys and other module
//! operations continue to use the compiled module; these traits do not replace
//! its execution path or introduce additional `TinyBus` methods.

use std::{fmt, future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A backend operation whose errors retain the host's classification.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// Stable categories callers can use without interpreting display text.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// The caller supplied invalid voice input.
    InvalidInput,
    /// No provider or usable backend credential is available.
    Unavailable,
    /// A signed-in session credential has expired or been rejected.
    SessionExpired,
    /// A backend API key has been rejected.
    ApiKeyRejected,
    /// The backend request could not be completed.
    Transport,
    /// The backend returned an invalid or oversized response.
    InvalidResponse,
}

/// A provider failure with a stable category and safe display detail.
///
/// Providers must remove credential echoes before populating the message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderError {
    /// Machine-readable failure category.
    pub kind: ProviderErrorKind,
    /// Human-readable detail with credentials removed.
    pub message: String,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

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

/// Cloud transport supplied by the host's backend layer.
///
/// Implementations own HTTP and credential lifecycle. The consuming voice
/// domain owns provider selection, local fallbacks and presentation. Speech
/// responses remain opaque JSON so existing response normalization can retain
/// its tolerant backend shape without relocating generic voice behavior here.
pub trait CloudVoiceProvider: Send + Sync {
    /// Transcribe encoded audio.
    ///
    /// # Errors
    /// Returns the host-classified input, authentication or transport error.
    fn transcribe(&self, request: TranscriptionRequest) -> ProviderFuture<'_, Transcription>;

    /// Synthesize reply audio and return the provider's response payload.
    ///
    /// # Errors
    /// Returns the host-classified input, authentication or transport error.
    fn synthesize(&self, request: SynthesisRequest) -> ProviderFuture<'_, Value>;

    /// Bootstrap a hosted realtime session.
    ///
    /// # Errors
    /// Returns the host-classified authentication, transport or response error.
    fn realtime(&self, request: RealtimeRequest) -> ProviderFuture<'_, RealtimeSession>;
}
