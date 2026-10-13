//! Pins the JSON shape of the recording-session vocabulary.
#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn session_values_keep_their_json_contract() {
    assert_eq!(
        serde_json::to_string(&SessionPhase::Transcribing).unwrap(),
        r#""transcribing""#
    );
    assert_eq!(
        serde_json::to_string(&GateVerdict::TooShort).unwrap(),
        r#""too_short""#
    );
    assert_eq!(
        serde_json::to_string(&UtteranceDisposition::Command {
            command: "open slack".into()
        })
        .unwrap(),
        r#"{"disposition":"command","command":"open slack"}"#
    );
    assert_eq!(
        serde_json::to_string(&UtteranceDisposition::BareWake).unwrap(),
        r#"{"disposition":"bare_wake"}"#
    );
}

#[test]
fn session_inputs_and_actions_keep_their_json_contract() {
    assert_eq!(
        serde_json::to_string(&SessionInput::SetupReady {
            stop_buffered: true
        })
        .unwrap(),
        r#"{"input":"setup_ready","stop_buffered":true}"#
    );
    assert_eq!(
        serde_json::from_str::<SessionInput>(r#"{"input":"released"}"#).unwrap(),
        SessionInput::Released
    );
    assert_eq!(
        serde_json::to_string(&SessionAction::DeferStop {
            generation: 3,
            after_ms: DEFERRED_STOP_MS
        })
        .unwrap(),
        r#"{"action":"defer_stop","generation":3,"after_ms":1500}"#
    );
    assert_eq!(
        serde_json::to_string(&SessionStatus {
            phase: SessionPhase::Idle,
            generation: 0
        })
        .unwrap(),
        r#"{"phase":"idle","generation":0}"#
    );
}
