//! Mechanical transport values retain ordered effects without debug authority leaks.

use super::*;
use crate::live::TranscriptRole;
use serde_json::json;

#[test]
fn transport_callback_wire_preserves_sequence_and_delivery_identity_while_debug_redacts_payloads()
-> Result<(), serde_json::Error> {
    let port = LiveTransportPort {
        bus_name: "ai.test.Transport".into(),
        object_path: "/test/Transport".into(),
        context_token: "private-token".into(),
    };
    let read = ReadLiveInputRequest {
        context_token: port.context_token.clone(),
        acknowledged_frame: 7,
    };
    let close = CloseLiveTransportRequest {
        context_token: port.context_token.clone(),
    };
    let delivery = DeliverLiveEffectsRequest {
        context_token: port.context_token.clone(),
        handle: LiveHandle {
            id: "opaque".into(),
        },
        delivery_id: "input-8".into(),
        outputs: vec![LiveOutput::Persist {
            role: TranscriptRole::User,
            text: "private-user-text".into(),
        }],
    };
    for debug in [
        format!("{port:?}"),
        format!("{read:?}"),
        format!("{close:?}"),
        format!("{delivery:?}"),
    ] {
        assert!(!debug.contains("private-token"));
        assert!(!debug.contains("private-user-text"));
    }
    assert_eq!(
        serde_json::to_value(&read)?,
        json!({"context_token":"private-token","acknowledged_frame":7})
    );
    assert_eq!(
        serde_json::to_value(&delivery)?,
        json!({"context_token":"private-token","handle":{"id":"opaque"},"delivery_id":"input-8","outputs":[{"kind":"persist","role":"user","text":"private-user-text"}]})
    );
    assert_eq!(
        serde_json::from_value::<DeliverLiveEffectsRequest>(serde_json::to_value(&delivery)?)?,
        delivery
    );
    Ok(())
}
