//! Pure extraction and classification of provider-relayed conversational text.

use serde_json::Value;
use tinyvoice_bus::relay::{PreparedRelayTurn, RelayTurnRequest};

fn content_to_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Extract the latest user prompt and replayable text, retaining relay semantics.
///
/// The supplied prefix is host policy. This function never executes a model,
/// copies provider system instructions, or emits progress events.
#[must_use]
pub fn prepare_turn(request: &RelayTurnRequest) -> PreparedRelayTurn {
    let prompt = request
        .messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| content_to_text(message.get("content")))
        .unwrap_or_default();
    let history = request
        .messages
        .iter()
        .filter_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            if !matches!(role, "user" | "assistant" | "agent") {
                return None;
            }
            let text = content_to_text(message.get("content"));
            (!text.trim().is_empty()).then(|| (role.to_owned(), text))
        })
        .collect();
    let readback_payload = prompt
        .trim_start()
        .strip_prefix(&request.readback_prefix)
        .map(|text| text.trim().to_owned());
    let content_free = !prompt.chars().any(char::is_alphanumeric);
    let arm_speak_back = readback_payload.is_none();
    PreparedRelayTurn {
        prompt,
        history,
        readback_payload,
        content_free,
        arm_speak_back,
        answerable: !content_free && arm_speak_back,
    }
}

/// Regressions for the migrated relay text contract.
#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
