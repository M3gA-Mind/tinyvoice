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
