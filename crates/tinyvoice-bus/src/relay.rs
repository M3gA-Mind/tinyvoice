//! Vocabulary for preparing provider-relayed conversational turns.

use serde::{Deserialize, Serialize};

/// Maximum serialized request size accepted by the compiled relay operation.
///
/// The prepared reply can retain text in the prompt, history and readback.
/// A four MiB aggregate input limit leaves room for those copies and the bus
/// envelope within TinyBus's sixteen MiB frame limit.
pub const MAX_RELAY_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// Messages and host policy used to prepare one relayed turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayTurnRequest {
    /// OpenAI-style messages, including malformed or unsupported content.
    pub messages: Vec<serde_json::Value>,
    /// Host-selected prefix identifying a verbatim readback request.
    pub readback_prefix: String,
}

/// Extracted text and classifications, without executing an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRelayTurn {
    /// Text of the last user message, or empty when none exists.
    pub prompt: String,
    /// Nonempty user, assistant and agent history in its original order.
    pub history: Vec<(String, String)>,
    /// Trimmed readback payload, including an explicitly empty payload.
    pub readback_payload: Option<String>,
    /// Whether the prompt contains no Unicode letter or digit.
    pub content_free: bool,
    /// Whether the caller should arm deferred-result readback.
    pub arm_speak_back: bool,
    /// Whether the prompt carries content and is not itself a readback.
    pub answerable: bool,
}
