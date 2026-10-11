//! Native provider and harness construction from explicit bus configuration.

use std::collections::HashSet;
use std::sync::Arc;

use tinyagents_harness::{
    context::{RunConfig, RunContext},
    runtime::AgentHarness,
};
use tinyagents_live::{
    LiveAgent, LiveAgentOptions,
    tinyliveagents::{
        AudioFormat, LiveConfig, LiveProvider, ToolDeclaration, TurnDetection, VadConfig,
        elevenlabs::ElevenLabsConvai,
        gemini::{GeminiLive, GeminiRelay},
        sarvam::SarvamCascade,
    },
};
use tinyvoice_bus::live::session::{LiveConfiguration, LiveConnection, LiveStartRequest};

use super::callback::CallbackTool;

/// Fully constructed portable provider/tool-loop inputs.
pub(super) struct Prepared {
    pub(super) agent: LiveAgent<(), ()>,
    pub(super) provider: Box<dyn LiveProvider>,
    pub(super) config: LiveConfig,
    pub(super) context: RunContext<()>,
    pub(super) options: LiveAgentOptions,
}

/// Bind only the exact declarations supplied by the host.
pub(super) fn prepare(
    request: &LiveStartRequest,
    connection: &tinybus::Connection,
) -> tinybus::Result<Prepared> {
    if request.tool_timeout_ms == 0
        || request.connect_timeout_ms == 0
        || request.max_tool_calls == 0
        || request.max_tool_calls > 10_000
        || request.connect_timeout_ms > 60_000
        || request.tool_timeout_ms > 1_800_000
    {
        return Err(tinybus::Error::failed(
            "live timeout and tool-call bounds must be positive and finite",
        ));
    }
    let mut names = HashSet::new();
    let mut harness = AgentHarness::<(), ()>::new();
    for declaration in &request.configuration.tools {
        if declaration.name.is_empty() || !names.insert(declaration.name.as_str()) {
            return Err(tinybus::Error::failed(
                "live tool names must be nonempty and unique",
            ));
        }
        harness.register_tool_dispatch(Arc::new(CallbackTool::new(
            declaration.clone(),
            request.tool_port.clone(),
            connection,
        )?));
    }
    let provider: Box<dyn LiveProvider> = match &request.connection {
        LiveConnection::Gemini { api_key } => Box::new(GeminiLive::new(api_key)),
        LiveConnection::GeminiRelay {
            ws_url,
            session_id,
            model,
        } => Box::new(GeminiRelay::connect_url(ws_url).with_session(session_id, model)),
        LiveConnection::ElevenLabs { signed_url } => {
            Box::new(ElevenLabsConvai::connect_url(signed_url))
        }
        LiveConnection::Sarvam { api_key } => Box::new(SarvamCascade::new(api_key)),
    };
    let max_calls = usize::try_from(request.max_tool_calls).map_err(|_| {
        tinybus::Error::failed("live tool-call bound is unsupported on this target")
    })?;
    Ok(Prepared {
        agent: LiveAgent::new(Arc::new(harness), Arc::new(())),
        provider,
        config: configuration(&request.configuration),
        context: RunContext::new(
            RunConfig::new(format!("live-{}", request.handle.id)).with_max_tool_calls(max_calls),
            (),
        ),
        options: LiveAgentOptions {
            tool_timeout: std::time::Duration::from_millis(request.tool_timeout_ms),
            include_deferred_tools: false,
        },
    })
}

fn configuration(config: &LiveConfiguration) -> LiveConfig {
    LiveConfig {
        model: config.model.clone(),
        system_instruction: config.system_instruction.clone(),
        voice: config.voice.clone(),
        language: config.language.clone(),
        first_message: config.first_message.clone(),
        input_format: AudioFormat::pcm16(config.input_sample_rate),
        tools: config
            .tools
            .iter()
            .map(|tool| {
                ToolDeclaration::new(&tool.name, &tool.description, tool.parameters.clone())
            })
            .collect(),
        input_transcription: config.input_transcription,
        output_transcription: config.output_transcription,
        vad: VadConfig {
            turn_detection: if config.manual_turn_detection {
                TurnDetection::Manual
            } else {
                TurnDetection::Server
            },
            silence_ms: config.silence_ms,
            threshold: config.speech_threshold,
        },
        provider_options: config.provider_options.clone(),
    }
}

#[cfg(test)]
#[path = "prepare_tests.rs"]
mod tests;
