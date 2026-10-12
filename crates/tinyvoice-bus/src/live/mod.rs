//! Portable live websocket control vocabulary.

pub mod session;
/// Mechanical client socket and persistence callbacks for module-owned pumping.
pub mod transport;

use serde::{Deserialize, Serialize};

/// A client control frame; audio travels separately as PCM16 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    /// First-frame connection request; identities remain opaque host metadata.
    Start {
        /// Optional host provider selection.
        #[serde(default)]
        provider: Option<String>,
        /// Optional host transcript identity.
        #[serde(default)]
        thread_id: Option<String>,
        /// Optional host approval-routing identity.
        #[serde(default)]
        client_id: Option<String>,
        /// Mono PCM16 input rate, defaulting to sixteen kHz.
        #[serde(default = "default_input_rate")]
        input_sample_rate: u32,
    },
    /// Typed user input.
    Text {
        /// Original user text.
        text: String,
    },
    /// Interrupt the current provider reply.
    Interrupt,
    /// Ask the provider to close.
    Stop,
}

fn default_input_rate() -> u32 {
    16_000
}

/// Transport facts supplied to the module's control parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransportFrame {
    /// UTF-8 JSON control input.
    Text {
        /// The original websocket text.
        text: String,
    },
    /// Mono PCM16 little-endian input.
    Audio {
        /// PCM16 bytes.
        pcm16: Vec<u8>,
    },
    /// The client disconnected.
    Closed,
    /// Ping, pong and other non-command frames.
    Other,
}

/// Parse one control frame under the current session phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareLiveInputRequest {
    /// Whether the connection still requires its first start frame.
    pub awaiting_start: bool,
    /// Original transport frame.
    pub frame: TransportFrame,
}

/// A parsed command; malformed later frames are deliberately ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreparedLiveInput {
    /// An accepted control command.
    Control(ClientFrame),
    /// A non-control transport action.
    Transport(LiveTransportAction),
}

/// A transport action requiring no JSON control parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveTransportAction {
    /// Forward input audio.
    Audio {
        /// Original PCM16 bytes.
        pcm16: Vec<u8>,
    },
    /// Close the provider sender.
    Closed,
    /// No provider action.
    Ignore,
}

/// Who a final transcript belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRole {
    /// Spoken or typed user input.
    User,
    /// Spoken assistant output.
    Agent,
}

/// A live server JSON frame; audio remains a separate binary transport action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// The provider connected.
    Ready {
        /// Provider session identity or the host fallback.
        session_id: Option<String>,
        /// Host-selected provider identifier.
        provider: String,
        /// Mono PCM16 output sample rate.
        output_sample_rate: u32,
        /// Opaque host transcript identity.
        thread_id: Option<String>,
    },
    /// Partial or final speech transcript.
    Transcript {
        /// User or agent speech.
        role: TranscriptRole,
        /// Original transcript text.
        text: String,
        /// Whether this completes the utterance.
        #[serde(rename = "final")]
        is_final: bool,
    },
    /// A provider tool call entered the harness.
    ToolStarted {
        /// Provider call identity.
        call_id: String,
        /// Declared tool name.
        name: String,
    },
    /// A tool call completed or was cancelled.
    ToolFinished {
        /// Provider call identity.
        call_id: String,
        /// Declared tool name.
        name: String,
        /// Whether the result was successful.
        ok: bool,
        /// Whether the provider cancelled the result.
        cancelled: bool,
    },
    /// Provider playback was interrupted.
    Interrupted,
    /// The provider completed its turn.
    TurnComplete,
    /// A provider or connection error.
    Error {
        /// Stable portable error classification.
        code: String,
        /// Provider diagnostic.
        message: String,
        /// Whether the provider is terminating.
        fatal: bool,
    },
    /// The provider connection ended.
    Closed {
        /// Established close diagnostic.
        reason: String,
    },
}
