//! Contract round trips preserve provenance while debug output redacts authority.

use super::*;
use serde_json::json;

#[test]
fn live_connection_debug_never_contains_keys_signed_urls_or_provider_identity() {
    for connection in [
        LiveConnection::Gemini {
            api_key: "secret-key".into(),
        },
        LiveConnection::Sarvam {
            api_key: "secret-key".into(),
        },
        LiveConnection::ElevenLabs {
            signed_url: "wss://host/?secret-key".into(),
        },
        LiveConnection::GeminiRelay {
            ws_url: "wss://host/?secret-key".into(),
            session_id: "private-provider-id".into(),
            model: "private-model".into(),
        },
    ] {
        let debug = format!("{connection:?}");
        assert!(!debug.contains("secret-key"));
        assert!(!debug.contains("private-provider-id"));
        assert!(!debug.contains("private-model"));
    }
}

#[test]
fn live_start_debug_redacts_configuration_credentials_and_context_but_wire_preserves_them()
-> Result<(), serde_json::Error> {
    let mut request: LiveStartRequest = serde_json::from_str(include_str!(
        "../../../tinyvoice-module/src/service/live/fixtures/start.json"
    ))?;
    request.configuration.system_instruction = Some("private-prompt".into());
    request.configuration.provider_options = json!({"credential":"secret-value"});
    request.tool_port.context_token = "private-authority".into();
    request.connection = LiveConnection::Gemini {
        api_key: "secret-value".into(),
    };
    let debug = format!("{request:?}");
    for private in ["private-prompt", "secret-value", "private-authority"] {
        assert!(!debug.contains(private));
    }
    assert!(debug.contains("16000"));
    assert_eq!(
        serde_json::from_value::<LiveStartRequest>(serde_json::to_value(&request)?)?,
        request
    );
    Ok(())
}

#[test]
fn tool_callback_wire_preserves_distinct_native_and_cancellation_identities_without_debug_payloads()
-> Result<(), serde_json::Error> {
    let execute = ExecuteLiveToolRequest {
        context_token: "private-authority".into(),
        invocation_id: "module-invocation".into(),
        call_id: "native-call".into(),
        name: "approved".into(),
        arguments: json!({"private":"user-input"}),
        prefer_markdown: true,
    };
    let wire = serde_json::to_value(&execute)?;
    assert_eq!(
        wire,
        json!({"context_token":"private-authority","invocation_id":"module-invocation","call_id":"native-call","name":"approved","arguments":{"private":"user-input"},"prefer_markdown":true})
    );
    let cancel = CancelLiveToolRequest {
        context_token: execute.context_token.clone(),
        invocation_id: execute.invocation_id.clone(),
        call_id: execute.call_id.clone(),
    };
    let finish = FinishLiveContextRequest {
        context_token: execute.context_token.clone(),
    };
    let result = ExecuteLiveToolReply {
        result: json!({"content":[{"type":"text","text":"private-result"}],"is_error":true}),
    };
    for debug in [
        format!("{execute:?}"),
        format!("{cancel:?}"),
        format!("{finish:?}"),
        format!("{result:?}"),
    ] {
        for private in ["private-authority", "user-input", "private-result"] {
            assert!(!debug.contains(private));
        }
    }
    assert_eq!(
        serde_json::to_value(&cancel)?,
        json!({"context_token":"private-authority","invocation_id":"module-invocation","call_id":"native-call"})
    );
    Ok(())
}
