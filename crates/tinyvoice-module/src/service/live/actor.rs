//! Owned provider pump with bounded output and bounded host-context cleanup.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::sync::Arc;
use std::time::Duration;
use tinyagents_live::{
    LiveAgentEvent, LiveAgentSession,
    tinyliveagents::{CloseReason, Error, LiveEvent},
};
use tinyvoice_bus::live::{
    ServerFrame,
    session::{
        FINISH_CALLBACK_METHOD, FinishLiveContextReply, FinishLiveContextRequest, LiveOutput,
        LiveStartRequest, LiveState, TOOL_CALLBACK_INTERFACE,
    },
};

use super::{
    prepare::Prepared,
    table::{Lease, MAX_AUDIO_BYTES, MAX_TEXT_BYTES},
};

const FINISH_TIMEOUT: Duration = Duration::from_secs(1);

/// Own the connection, native loop and cleanup for one accepted start request.
pub(super) async fn run(
    lease: Arc<Lease>,
    request: LiveStartRequest,
    prepared: Prepared,
    connection: tinybus::Connection,
) {
    let connected = tokio::select! {
        biased;
        () = lease.wait_cancelled() => None,
        result = tokio::time::timeout(
            Duration::from_millis(request.connect_timeout_ms),
            prepared.agent.start(
                prepared.provider.as_ref(), prepared.config, prepared.context, prepared.options,
            ),
        ) => Some(result.unwrap_or(Err(Error::Timeout))),
    };
    match connected {
        Some(Ok(mut session)) => {
            if let Ok(mut phase) = lease.phase.lock() {
                phase.sender = Some(session.sender());
                phase.state = LiveState::Running;
            }
            tokio::select! {
                biased;
                () = lease.wait_cancelled() => {}
                () = forward(&lease, &mut session, &request) => {}
            }
            // Drop stops TinyAgentsLive's worker and provider event driver before
            // asking the host to cancel its independently owned callback work.
            drop(session);
        }
        Some(Err(error)) => {
            let outputs = super::projection::map_event(
                LiveAgentEvent::Live(LiveEvent::Closed(CloseReason::Error(error))),
                &request.session_id,
                &request.provider,
                &request.thread_id,
            );
            let _ = lease.outputs.send(outputs).await;
        }
        None => {}
    }
    let confirmed = finish_context(&connection, &request).await;
    if let Ok(mut phase) = lease.phase.lock() {
        phase.sender = None;
        phase.state = LiveState::Closed;
        phase.cleanup_confirmed = confirmed;
    }
}

/// Drain native events in order; output backpressure never blocks input methods.
async fn forward(lease: &Lease, session: &mut LiveAgentSession, request: &LiveStartRequest) {
    while let Some(event) = session.recv().await {
        let terminal = matches!(event, LiveAgentEvent::Live(LiveEvent::Closed(_)));
        match event {
            LiveAgentEvent::Live(LiveEvent::Audio(pcm)) => {
                for chunk in pcm.chunks(MAX_AUDIO_BYTES) {
                    if lease
                        .outputs
                        .send(vec![LiveOutput::Audio {
                            pcm16_base64: BASE64.encode(chunk),
                        }])
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            event => {
                if oversized(&event) {
                    let outputs = vec![
                        LiveOutput::Json {
                            frame: ServerFrame::Error {
                                code: "invalid_response".into(),
                                message: "live provider text exceeds the bounded output limit"
                                    .into(),
                                fatal: true,
                            },
                        },
                        LiveOutput::Json {
                            frame: ServerFrame::Closed {
                                reason: "error".into(),
                            },
                        },
                    ];
                    let _ = lease.outputs.send(outputs).await;
                    return;
                }
                let outputs = super::projection::map_event(
                    event,
                    &request.session_id,
                    &request.provider,
                    &request.thread_id,
                );
                if !outputs.is_empty() && lease.outputs.send(outputs).await.is_err() {
                    return;
                }
            }
        }
        if terminal {
            return;
        }
    }
}

fn oversized(event: &LiveAgentEvent) -> bool {
    match event {
        LiveAgentEvent::Live(
            LiveEvent::InputTranscript { text, .. } | LiveEvent::OutputTranscript { text, .. },
        ) => text.len() > MAX_TEXT_BYTES,
        LiveAgentEvent::ToolStarted { call_id, name }
        | LiveAgentEvent::ToolFinished { call_id, name, .. } => {
            call_id.len() > MAX_TEXT_BYTES || name.len() > MAX_TEXT_BYTES
        }
        _ => false,
    }
}

/// Stop waiting after the deadline even if a host callback remains unresponsive.
///
/// The host owns its work: it must also cancel it when its context lease drops.
/// This notification revokes a known context; it never creates one.
async fn finish_context(connection: &tinybus::Connection, request: &LiveStartRequest) -> bool {
    let Ok(proxy) = connection.proxy(
        request.tool_port.bus_name.as_str(),
        request.tool_port.object_path.as_str(),
        TOOL_CALLBACK_INTERFACE,
    ) else {
        return false;
    };
    let call = proxy.call::<FinishLiveContextReply>(
        FINISH_CALLBACK_METHOD,
        (FinishLiveContextRequest {
            context_token: request.tool_port.context_token.clone(),
        },),
    );
    matches!(tokio::time::timeout(FINISH_TIMEOUT, call).await, Ok(Ok(reply)) if reply.finished)
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
