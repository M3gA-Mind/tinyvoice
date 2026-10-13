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
