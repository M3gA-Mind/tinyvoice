//! Relay wire provenance remains distinct from product identity and debug payloads.

use super::*;
use serde_json::json;

#[test]
fn relay_callbacks_preserve_reserved_authority_and_ordered_effects_without_debug_payloads()
-> Result<(), serde_json::Error> {
    let port = RelayHostPort {
        bus_name: "ai.test.Relay".into(),
        object_path: "/test/Relay".into(),
        context_token: "private-context".into(),
    };
    let invocation = RelayInvocationRequest {
        context_token: port.context_token.clone(),
        invocation_id: "module-reservation".into(),
    };
    let start = RelayStartRequest {
        handle: RelayHandle {
            id: invocation.invocation_id.clone(),
        },
        port: port.clone(),
        acknowledgment_deadline_ms: 8000,
        turn_timeout_ms: 120_000,
        empty_reply_text: "private-pause".into(),
        deadline_reply_text: "private-ack".into(),
        lost_reply_text: "private-lost".into(),
    };
    let read = ReadRelayBrainRequest {
        context_token: port.context_token.clone(),
        invocation_id: invocation.invocation_id.clone(),
        acknowledged_event: 7,
    };
    let outcome = RelayOutcome::Success {
        text: "private-result".into(),
    };
    let event = RelayBrainEvent::Complete {
        outcome: outcome.clone(),
    };
    let reply = ReadRelayBrainReply {
        sequence: 8,
        event: Some(event.clone()),
    };
    let effect = RelayEffect::Deferred {
        outcome: outcome.clone(),
    };
    let apply = ApplyRelayEffectsRequest {
        context_token: port.context_token.clone(),
        invocation_id: invocation.invocation_id.clone(),
        delivery_id: 4,
        effects: vec![
            RelayEffect::Delta {
                text: "private-delta".into(),
            },
            RelayEffect::Done,
        ],
    };
    for debug in [
        format!("{port:?}"),
        format!("{invocation:?}"),
        format!("{start:?}"),
        format!("{read:?}"),
        format!("{outcome:?}"),
        format!("{event:?}"),
        format!("{reply:?}"),
        format!("{effect:?}"),
        format!("{apply:?}"),
    ] {
        for private in [
            "private-context",
            "private-result",
            "private-delta",
            "private-pause",
            "private-ack",
            "private-lost",
        ] {
            assert!(!debug.contains(private));
        }
    }
    assert_eq!(
        serde_json::to_value(&read)?,
        json!({"context_token":"private-context","invocation_id":"module-reservation","acknowledged_event":7})
    );
    assert_eq!(
        serde_json::to_value(&reply)?,
        json!({"sequence":8,"event":{"kind":"complete","outcome":{"kind":"success","text":"private-result"}}})
    );
    assert_eq!(
        serde_json::to_value(&apply)?,
        json!({"context_token":"private-context","invocation_id":"module-reservation","delivery_id":4,"effects":[{"kind":"delta","text":"private-delta"},{"kind":"done"}]})
    );
    assert_eq!(
        serde_json::from_value::<ApplyRelayEffectsRequest>(serde_json::to_value(&apply)?)?,
        apply
    );
    assert_eq!(
        serde_json::from_value::<RelayStartRequest>(serde_json::to_value(&start)?)?,
        start
    );
    Ok(())
}
