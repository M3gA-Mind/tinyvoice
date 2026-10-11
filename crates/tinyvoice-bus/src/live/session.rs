//! Bounded compiled live session and host tool callback vocabulary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ServerFrame, TranscriptRole};

/// Fixed interface served by a host's policy-aware tool callback.
pub const TOOL_CALLBACK_INTERFACE: &str = "ai.tinyhumans.tinyvoice.LiveTools";
/// The only method invoked on a live tool callback endpoint.
pub const TOOL_CALLBACK_METHOD: &str = "ExecuteLiveTool";

/// An opaque live lease, reserved before provider connection begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveHandle {
    /// Unpredictable module-generated lease identity.
    pub id: u128,
}

/// Connection state, independent of host permissions or UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveState {
    /// Reserved before provider startup.
    Reserved,
    /// Provider connection is still opening.
    Starting,
    /// Ready for commands.
    Running,
    /// Terminal events remain readable.
    Closed,
}

/// Result of reserving or changing a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveStatus {
    /// The referenced lease.
    pub handle: LiveHandle,
    /// Current lifecycle state.
    pub state: LiveState,
}

/// Ordered effects projected from native provider events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveOutput {
    /// Send a JSON frame to the client.
    Json {
        /// Stable client-facing frame.
        frame: ServerFrame,
    },
    /// Send PCM16 bytes as a binary client frame.
    Audio {
        /// Base64 transports bytes within the bounded JSON bus frame.
        pcm16_base64: String,
    },
    /// Persist final speech after its JSON frame has been delivered.
    Persist {
        /// Transcript speaker.
        role: TranscriptRole,
        /// Original transcript text.
        text: String,
    },
}

/// Replayable bounded event batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveBatch {
    /// Lease being read.
    pub handle: LiveHandle,
    /// Monotonic batch identity, retained until acknowledged.
    pub batch: u64,
    /// Current lifecycle state.
    pub state: LiveState,
    /// Ordered effects; final transcripts retain frame-before-persist order.
    pub outputs: Vec<LiveOutput>,
}

/// Read a session, optionally acknowledging the preceding delivered batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveReadRequest {
    /// Opaque lease identity.
    pub handle: LiveHandle,
    /// The last batch fully applied by the host.
    pub acknowledged_batch: Option<u64>,
}

/// A host-approved declaration; execution still consults current host policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveToolDeclaration {
    /// Canonical tool name.
    pub name: String,
    /// Provider-visible description.
    pub description: String,
    /// Provider-visible JSON argument schema.
    pub parameters: Value,
    /// Preserve the host tool's compact result rendering preference.
    #[serde(default)]
    pub prefer_markdown: bool,
}

/// Port into the host's saved policy and approval context.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveToolPort {
    /// Host bus name.
    pub bus_name: String,
    /// Dedicated host callback object.
    pub object_path: String,
    /// Opaque authority token checked by the host, never model arguments.
    pub context_token: String,
}

impl std::fmt::Debug for LiveToolPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveToolPort")
            .field("bus_name", &self.bus_name)
            .field("object_path", &self.object_path)
            .field("context_token", &"[redacted]")
            .finish()
    }
}

/// One call to the host's current policy-aware harness.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecuteLiveToolRequest {
    /// Saved host authority, checked independently of the declaration.
    pub context_token: String,
    /// Admitted provider call identity.
    pub call_id: String,
    /// Declared tool name.
    pub name: String,
    /// Provider-supplied arguments.
    pub arguments: Value,
    /// Requested rendering; the host still owns output screening.
    pub prefer_markdown: bool,
}

impl std::fmt::Debug for ExecuteLiveToolRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecuteLiveToolRequest")
            .field("context_token", &"[redacted]")
            .field("call_id", &self.call_id)
            .field("name", &self.name)
            .field("arguments", &"[redacted]")
            .field("prefer_markdown", &self.prefer_markdown)
            .finish()
    }
}

/// A canonical tool result serialized mechanically by the host.
///
/// It uses TinyTools' complete result JSON, including its reported-error flag.
/// The module decodes that existing vocabulary rather than inventing success.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecuteLiveToolReply {
    /// Serialized canonical tool result.
    pub result: Value,
}

impl std::fmt::Debug for ExecuteLiveToolReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecuteLiveToolReply")
            .finish_non_exhaustive()
    }
}

/// Provider connection material supplied by the host after credential lookup.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveConnection {
    /// Direct Gemini Live connection.
    Gemini {
        /// Host-provided API key.
        api_key: String,
    },
    /// Prepared Gemini relay connection.
    GeminiRelay {
        /// Prepared websocket URL; may contain a credential.
        ws_url: String,
        /// Prepared provider session identity.
        session_id: String,
        /// Prepared provider model.
        model: String,
    },
    /// Prepared ElevenLabs Conversational AI connection.
    ElevenLabs {
        /// Prepared signed websocket URL.
        signed_url: String,
    },
    /// Direct Sarvam chained connection.
    Sarvam {
        /// Host-provided API key.
        api_key: String,
    },
}

impl std::fmt::Debug for LiveConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::Gemini { .. } => "gemini",
            Self::GeminiRelay { .. } => "gemini_relay",
            Self::ElevenLabs { .. } => "eleven_labs",
            Self::Sarvam { .. } => "sarvam",
        };
        f.debug_struct("LiveConnection")
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

/// Provider configuration for mono PCM16 live sessions.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveConfiguration {
    /// Optional provider model.
    pub model: Option<String>,
    /// Host-composed system instruction.
    pub system_instruction: Option<String>,
    /// Optional voice name.
    pub voice: Option<String>,
    /// Optional BCP-47 language.
    pub language: Option<String>,
    /// Optional initial spoken message.
    pub first_message: Option<String>,
    /// Input PCM16 sample rate.
    pub input_sample_rate: u32,
    /// Explicit host-approved tool declarations.
    pub tools: Vec<LiveToolDeclaration>,
    /// Whether user transcripts are requested.
    pub input_transcription: bool,
    /// Whether agent transcripts are requested.
    pub output_transcription: bool,
    /// Use host activity markers instead of provider VAD.
    pub manual_turn_detection: bool,
    /// Optional trailing silence in milliseconds.
    pub silence_ms: Option<u32>,
    /// Optional speech probability threshold.
    pub speech_threshold: Option<f32>,
    /// Provider-specific settings, including prepared hosted identity fields.
    pub provider_options: Value,
}

impl std::fmt::Debug for LiveConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveConfiguration")
            .field("input_sample_rate", &self.input_sample_rate)
            .field("tools", &self.tools.len())
            .finish_non_exhaustive()
    }
}

/// Begin connection on a previously reserved handle.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveStartRequest {
    /// Reserved session lease.
    pub handle: LiveHandle,
    /// Prepared provider credentials.
    pub connection: LiveConnection,
    /// Explicit provider configuration.
    pub configuration: LiveConfiguration,
    /// Policy-aware host tool executor port.
    pub tool_port: LiveToolPort,
    /// Fallback session identity presented to the client.
    pub session_id: String,
    /// Host provider label presented to the client.
    pub provider: String,
    /// Host transcript identity presented to the client.
    pub thread_id: String,
    /// Host-selected outer tool-call timeout in milliseconds.
    pub tool_timeout_ms: u64,
    /// Host-selected connection timeout in milliseconds.
    pub connect_timeout_ms: u64,
}

impl std::fmt::Debug for LiveStartRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveStartRequest")
            .field("handle", &self.handle)
            .field("connection", &self.connection)
            .field("configuration", &self.configuration)
            .field("tool_port", &self.tool_port)
            .finish_non_exhaustive()
    }
}

/// Session command transported separately from client framing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveCommand {
    /// Forward mono PCM16 audio.
    Audio {
        /// Base64-encoded PCM16 bytes.
        pcm16_base64: String,
    },
    /// Forward typed input; host persistence precedes this call.
    Text {
        /// Original input text.
        text: String,
    },
    /// Interrupt provider playback.
    Interrupt,
    /// Request graceful provider closure.
    Stop,
}

/// Send a command to a running session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSendRequest {
    /// Running lease.
    pub handle: LiveHandle,
    /// Input command.
    pub command: LiveCommand,
}

/// Stop, release or inspect a previously reserved session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveHandleRequest {
    /// Opaque session lease.
    pub handle: LiveHandle,
}
