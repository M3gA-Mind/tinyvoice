//! A host can supply cloud transport through the minimal provider contract.

use std::task::{Context, Poll, Waker};
use tinyvoice_bus::provider::{
    CloudVoiceProvider, ProviderFuture, RealtimeRequest, RealtimeSession, SynthesisRequest,
    Transcription, TranscriptionRequest,
};

#[derive(Debug)]
struct HostProvider;

impl CloudVoiceProvider for HostProvider {
    fn transcribe(&self, request: TranscriptionRequest) -> ProviderFuture<'_, Transcription> {
        Box::pin(std::future::ready(if request.audio_base64.is_empty() {
            Err("audio is required".into())
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
        Poll::Ready(Err("audio is required".into()))
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
