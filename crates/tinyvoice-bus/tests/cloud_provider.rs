//! A host can supply cloud transport through the minimal provider contract.

use std::task::{Context, Poll, Waker};
use tinyvoice_bus::provider::{
    CloudVoiceProvider, ProviderError, ProviderErrorKind, ProviderFuture, RealtimeRequest,
    RealtimeSession, SynthesisRequest, Transcription, TranscriptionRequest,
};

#[derive(Debug)]
struct HostProvider;

impl CloudVoiceProvider for HostProvider {
    fn transcribe(&self, request: TranscriptionRequest) -> ProviderFuture<'_, Transcription> {
        Box::pin(std::future::ready(if request.audio_base64.is_empty() {
            Err(ProviderError {
                kind: ProviderErrorKind::InvalidInput,
                message: "audio is required".into(),
            })
        } else {
            Ok(Transcription {
                text: "spoken words".into(),
            })
        }))
    }

    fn synthesize(&self, request: SynthesisRequest) -> ProviderFuture<'_, serde_json::Value> {
        Box::pin(std::future::ready(Ok(
            serde_json::json!({ "audio": request.text, "cues": [] }),
        )))
    }

    fn realtime(&self, request: RealtimeRequest) -> ProviderFuture<'_, RealtimeSession> {
        Box::pin(std::future::ready(Ok(match request {
            RealtimeRequest::VoiceAgent => RealtimeSession::VoiceAgent {
                signed_url: "wss://voice.invalid/session".into(),
                agent_id: "agent".into(),
                user_token: "short-lived".into(),
            },
            RealtimeRequest::GeminiLive { .. } => RealtimeSession::GeminiLive {
                ws_url: "wss://voice.invalid/relay".into(),
                session_id: "session".into(),
                model: "gemini".into(),
            },
        })))
    }
}

#[test]
fn a_host_provider_dispatches_without_voice_implementation_or_runtime() {
    let provider: Box<dyn CloudVoiceProvider> = Box::new(HostProvider);
    let mut context = Context::from_waker(Waker::noop());
    let mut request = TranscriptionRequest::default();
    assert_eq!(
        provider
            .transcribe(request.clone())
            .as_mut()
            .poll(&mut context),
        Poll::Ready(Err(ProviderError {
            kind: ProviderErrorKind::InvalidInput,
            message: "audio is required".into()
        }))
    );
    request.audio_base64 = "YXVkaW8=".into();
    assert_eq!(
        provider.transcribe(request).as_mut().poll(&mut context),
        Poll::Ready(Ok(Transcription {
            text: "spoken words".into()
        }))
    );
    let speech = provider
        .synthesize(SynthesisRequest {
            text: "reply".into(),
            ..SynthesisRequest::default()
        })
        .as_mut()
        .poll(&mut context);
    assert_eq!(
        speech,
        Poll::Ready(Ok(serde_json::json!({ "audio": "reply", "cues": [] })))
    );
    let session = provider
        .realtime(RealtimeRequest::VoiceAgent)
        .as_mut()
        .poll(&mut context);
    assert_eq!(
        session,
        Poll::Ready(Ok(RealtimeSession::VoiceAgent {
            signed_url: "wss://voice.invalid/session".into(),
            agent_id: "agent".into(),
            user_token: "short-lived".into()
        }))
    );
    let relay = provider
        .realtime(RealtimeRequest::GeminiLive {
            options: serde_json::json!({"model": "gemini"}),
        })
        .as_mut()
        .poll(&mut context);
    assert_eq!(
        relay,
        Poll::Ready(Ok(RealtimeSession::GeminiLive {
            ws_url: "wss://voice.invalid/relay".into(),
            session_id: "session".into(),
            model: "gemini".into()
        }))
    );
}

#[test]
fn provider_values_preserve_audio_hints_and_session_metadata() -> Result<(), serde_json::Error> {
    let input = TranscriptionRequest {
        audio_base64: "YXVkaW8=".into(),
        model: Some("model".into()),
        language: Some("en".into()),
        mime_type: Some("audio/wav".into()),
        file_name: Some("voice.wav".into()),
    };
    assert_eq!(
        serde_json::from_value::<TranscriptionRequest>(serde_json::to_value(&input)?)?,
        input
    );
    let speech = SynthesisRequest {
        text: "reply".into(),
        voice_id: Some("voice".into()),
        model_id: Some("speech".into()),
        output_format: Some("mp3".into()),
        voice_settings: Some(serde_json::json!({"stability": 0.5, "future_option": true})),
    };
    assert_eq!(
        serde_json::from_value::<SynthesisRequest>(serde_json::to_value(&speech)?)?,
        speech
    );
    for request in [
        RealtimeRequest::VoiceAgent,
        RealtimeRequest::GeminiLive {
            options: serde_json::json!({"voice": "Puck"}),
        },
    ] {
        assert_eq!(
            serde_json::from_value::<RealtimeRequest>(serde_json::to_value(&request)?)?,
            request
        );
    }
    for session in [
        RealtimeSession::VoiceAgent {
            signed_url: "wss://voice.invalid/signed".into(),
            agent_id: "agent".into(),
            user_token: "short-lived".into(),
        },
        RealtimeSession::GeminiLive {
            ws_url: "wss://voice.invalid/relay".into(),
            session_id: "session".into(),
            model: "gemini".into(),
        },
    ] {
        assert_eq!(
            serde_json::from_value::<RealtimeSession>(serde_json::to_value(&session)?)?,
            session
        );
    }
    assert!(
        serde_json::from_value::<RealtimeSession>(serde_json::json!({"kind": "unknown"})).is_err()
    );
    Ok(())
}

#[test]
fn realtime_debug_does_not_expose_connection_credentials() {
    let voice = RealtimeSession::VoiceAgent {
        signed_url: "wss://voice.invalid/?ticket=secret-ticket".into(),
        agent_id: "agent".into(),
        user_token: "secret-user-token".into(),
    };
    let gemini = RealtimeSession::GeminiLive {
        ws_url: "wss://voice.invalid/?ticket=secret-relay-ticket".into(),
        session_id: "session".into(),
        model: "gemini".into(),
    };
    for session in [voice, gemini] {
        let debug = format!("{session:?}");
        assert!(!debug.contains("secret"), "credentials exposed: {debug}");
        assert!(debug.contains("[REDACTED]"));
    }
}

#[test]
fn provider_errors_preserve_categories_independently_of_display_text()
-> Result<(), serde_json::Error> {
    use tinyvoice_bus::provider::{ProviderError, ProviderErrorKind};
    for kind in [
        ProviderErrorKind::InvalidInput,
        ProviderErrorKind::Unavailable,
        ProviderErrorKind::SessionExpired,
        ProviderErrorKind::ApiKeyRejected,
        ProviderErrorKind::Transport,
        ProviderErrorKind::InvalidResponse,
    ] {
        let error = ProviderError {
            kind,
            message: "same human-readable detail".into(),
        };
        let decoded: ProviderError = serde_json::from_value(serde_json::to_value(&error)?)?;
        assert_eq!(decoded.kind, kind);
        assert_eq!(decoded.to_string(), "same human-readable detail");
    }
    Ok(())
}
