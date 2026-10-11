//! Provider-event projection preserving client wire and transcript ordering.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use tinyagents_live::{
    LiveAgentEvent,
    tinyliveagents::{CloseReason, Error, LiveEvent},
};
use tinyvoice_bus::live::session::LiveOutput;
use tinyvoice_bus::live::{ServerFrame, TranscriptRole};

/// Convert one native event without applying host persistence or delivery policy.
pub(super) fn map_event(
    event: LiveAgentEvent,
    session_id: &str,
    provider: &str,
    thread_id: &str,
) -> Vec<LiveOutput> {
    match event {
        LiveAgentEvent::ToolStarted { call_id, name } => {
            vec![json(ServerFrame::ToolStarted { call_id, name })]
        }
        LiveAgentEvent::ToolFinished {
            call_id,
            name,
            is_error,
            cancelled,
            ..
        } => vec![json(ServerFrame::ToolFinished {
            call_id,
            name,
            ok: !is_error,
            cancelled,
        })],
        LiveAgentEvent::Live(event) => match event {
            LiveEvent::Ready(info) => vec![json(ServerFrame::Ready {
                session_id: Some(info.session_id.unwrap_or_else(|| session_id.to_string())),
                provider: provider.to_string(),
                output_sample_rate: info.output_format.sample_rate,
                thread_id: Some(thread_id.to_string()),
            })],
            LiveEvent::Audio(pcm) => vec![LiveOutput::Audio {
                pcm16_base64: BASE64.encode(pcm),
            }],
            LiveEvent::InputTranscript { text, is_final } => {
                transcript(TranscriptRole::User, text, is_final)
            }
            LiveEvent::OutputTranscript { text, is_final } => {
                transcript(TranscriptRole::Agent, text, is_final)
            }
            LiveEvent::Interrupted => vec![json(ServerFrame::Interrupted)],
            LiveEvent::TurnComplete { .. } => vec![json(ServerFrame::TurnComplete)],
            LiveEvent::Error { error, fatal } => {
                let error = project_error(&error);
                vec![json(ServerFrame::Error {
                    code: error.0.to_string(),
                    message: error.1,
                    fatal,
                })]
            }
            LiveEvent::Closed(reason) => {
                let mut out = Vec::new();
                let reason = match reason {
                    CloseReason::Client => "client".to_string(),
                    CloseReason::Remote { code, reason } => match code {
                        Some(code) => format!("remote ({code}) {reason}").trim().to_string(),
                        None => "remote".to_string(),
                    },
                    CloseReason::Error(error) => {
                        let error = project_error(&error);
                        out.push(json(ServerFrame::Error {
                            code: error.0.to_string(),
                            message: error.1,
                            fatal: true,
                        }));
                        "error".to_string()
                    }
                    _ => "closed".to_string(),
                };
                out.push(json(ServerFrame::Closed { reason }));
                out
            }
            // Tool calls are reported through ToolStarted/ToolFinished; the
            // rest has no client-visible meaning.
            _ => Vec::new(),
        },
    }
}

fn transcript(role: TranscriptRole, text: String, is_final: bool) -> Vec<LiveOutput> {
    let mut out = vec![json(ServerFrame::Transcript {
        role,
        text: text.clone(),
        is_final,
    })];
    if is_final {
        out.push(LiveOutput::Persist { role, text });
    }
    out
}

fn json(frame: ServerFrame) -> LiveOutput {
    LiveOutput::Json { frame }
}

fn project_error(error: &Error) -> (&'static str, String) {
    let code = match error {
        Error::Unauthorized => "unauthorized",
        Error::InsufficientCredits => "insufficient_credits",
        Error::RateLimited => "rate_limited",
        Error::Timeout => "timeout",
        Error::InvalidConfig(_) => "invalid_request",
        Error::Connect(_) => "connect",
        _ => "provider",
    };
    (code, error.to_string())
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
