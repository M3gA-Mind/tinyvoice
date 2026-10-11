//! Real loaded-module duplex, tool-authority and cleanup verification.

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::io;
use std::sync::Arc;
use tinyvoice_bus::live::{
    ServerFrame, TranscriptRole, TransportFrame,
    session::{
        ExecuteLiveToolReply, ExecuteLiveToolRequest, FinishLiveContextReply,
        FinishLiveContextRequest, LiveBatch, LiveCommand, LiveHandle, LiveHandleRequest,
        LiveOutput, LiveReadRequest, LiveSendRequest, LiveStartRequest, LiveState, LiveStatus,
    },
    transport::{
        CloseLiveTransportReply, CloseLiveTransportRequest, DeliverLiveEffectsReply,
        DeliverLiveEffectsRequest, LiveBindTransportRequest, LiveTransportPort, ReadLiveInputReply,
        ReadLiveInputRequest,
    },
};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct Host {
    calls: Arc<Mutex<Vec<ExecuteLiveToolRequest>>>,
    finishes: Arc<Mutex<usize>>,
}

#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.LiveTools")]
impl Host {
    async fn execute_live_tool(
        &self,
        request: ExecuteLiveToolRequest,
    ) -> tinybus::Result<ExecuteLiveToolReply> {
        if request.context_token != "known-live-context" || request.name != "approved" {
            return Err(tinybus::Error::failed("no current executor"));
        }
        self.calls.lock().await.push(request);
        // A previously declared tool is now revoked by the current host policy.
        Ok(ExecuteLiveToolReply {
            result: json!({"content": [{"type": "text", "text": "denied by current policy"}], "is_error": true}),
        })
    }
    async fn finish_live_context(
        &self,
        request: FinishLiveContextRequest,
    ) -> tinybus::Result<FinishLiveContextReply> {
        if request.context_token != "known-live-context" {
            return Ok(FinishLiveContextReply { finished: false });
        }
        *self.finishes.lock().await = 1;
        Ok(FinishLiveContextReply { finished: true })
    }
}

type CheckResult<T> = Result<T, Box<dyn std::error::Error>>;

pub(super) async fn verify_runtime(
    proxy: &tinybus::Proxy,
    client: &tinybus::Connection,
) -> CheckResult<()> {
    let host = Host::default();
    client
        .serve_at("/test/LiveTools".try_into()?, host.clone())
        .await?;
    client.request_name("ai.test.LiveTools").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let provider = tokio::spawn(scripted_provider(listener));
    let reserved: LiveStatus = proxy.call("LiveReserve", ()).await?;
    let request: LiveStartRequest = serde_json::from_value(json!({
        "handle": reserved.handle,
        "connection": {"kind": "gemini_relay", "ws_url": format!("ws://{address}"), "session_id": "provider-session", "model": "fixture-model"},
        "configuration": {"model": null, "system_instruction": null, "voice": null, "language": null, "first_message": null, "input_sample_rate": 16000,
            "tools": [{"name": "approved", "description": "host declaration", "parameters": {"type": "object"}, "prefer_markdown": false}],
            "input_transcription": true, "output_transcription": true, "manual_turn_detection": false, "silence_ms": null, "speech_threshold": null, "provider_options": {}},
        "tool_port": {"bus_name": "ai.test.LiveTools", "object_path": "/test/LiveTools", "context_token": "known-live-context"},
        "session_id": "fallback-session", "provider": "fixture-provider", "thread_id": "host-thread", "max_tool_calls": 8, "tool_timeout_ms": 1000, "connect_timeout_ms": 1000
    }))?;
    let started: LiveStatus = proxy.call("LiveStart", (request.clone(),)).await?;
    if started.state != LiveState::Starting {
        return Err(io::Error::other("start did not admit asynchronously").into());
    }
    let retry: LiveStatus = proxy.call("LiveStart", (request.clone(),)).await?;
    if retry.handle != started.handle {
        return Err(io::Error::other("start retry changed handle").into());
    }
    let outputs = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        collect_outputs(proxy, &reserved.handle),
    )
    .await??;
    provider.await??;
    check_outputs(&outputs, &host).await?;
    let released: LiveStatus = proxy
        .call(
            "LiveRelease",
            (LiveHandleRequest {
                handle: reserved.handle.clone(),
            },),
        )
        .await?;
    if released.state != LiveState::Closed || *host.finishes.lock().await != 1 {
        return Err(io::Error::other("release did not finish known context").into());
    }
    let _: LiveStatus = proxy
        .call(
            "LiveRelease",
            (LiveHandleRequest {
                handle: reserved.handle,
            },),
        )
        .await?;
    verify_bound_transport(proxy, client, &host, request).await?;
    let _: () = proxy.call("LiveShutdown", ()).await?;
    if proxy.call::<LiveStatus>("LiveReserve", ()).await.is_ok() {
        return Err(io::Error::other("shutdown reopened admission").into());
    }
    Ok(())
}

async fn collect_outputs(
    proxy: &tinybus::Proxy,
    handle: &LiveHandle,
) -> tinybus::Result<Vec<LiveOutput>> {
    let mut outputs = Vec::new();
    let mut ack = None;
    let mut sent = false;
    loop {
        let batch: LiveBatch = proxy
            .call(
                "LiveRead",
                (LiveReadRequest {
                    handle: handle.clone(),
                    acknowledged_batch: ack,
                },),
            )
            .await?;
        if !batch.outputs.is_empty() {
            let replay: LiveBatch = proxy
                .call(
                    "LiveRead",
                    (LiveReadRequest {
                        handle: handle.clone(),
                        acknowledged_batch: None,
                    },),
                )
                .await?;
            if replay != batch {
                return Err(tinybus::Error::failed("unacknowledged batch changed"));
            }
            ack = Some(batch.batch);
        }
        if !sent && batch.state == LiveState::Running {
            let _: LiveStatus = proxy
                .call(
                    "LiveSend",
                    (LiveSendRequest {
                        handle: handle.clone(),
                        command: LiveCommand::Text {
                            text: "hello".into(),
                        },
                    },),
                )
                .await?;
            sent = true;
        }
        let terminal = batch.outputs.iter().any(|output| {
            matches!(
                output,
                LiveOutput::Json {
                    frame: ServerFrame::Closed { .. }
                }
            )
        });
        outputs.extend(batch.outputs);
        if terminal {
            return Ok(outputs);
        }
        tokio::task::yield_now().await;
    }
}

async fn scripted_provider(listener: tokio::net::TcpListener) -> Result<(), io::Error> {
    let (stream, _) = listener.accept().await?;
    let mut socket = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(io::Error::other)?;
    let message = socket
        .next()
        .await
        .ok_or_else(|| io::Error::other("provider disconnected"))?
        .map_err(io::Error::other)?;
    if !message
        .to_text()
        .map_err(io::Error::other)?
        .contains("hello")
    {
        return Err(io::Error::other("text command was not forwarded"));
    }
    socket.send(tokio_tungstenite::tungstenite::Message::Text(json!({"serverContent": {"inputTranscription": {"text": "heard"}, "outputTranscription": {"text": "reply"}, "turnComplete": true}}).to_string().into())).await.map_err(io::Error::other)?;
    socket.send(tokio_tungstenite::tungstenite::Message::Text(json!({"toolCall": {"functionCalls": [{"id": "native-7", "name": "approved", "args": {"input": "safe"}}, {"id": "unknown-8", "name": "undeclared", "args": {}}]}}).to_string().into())).await.map_err(io::Error::other)?;
    let mut response_ids = Vec::new();
    while response_ids.len() < 2 {
        let message = socket
            .next()
            .await
            .ok_or_else(|| io::Error::other("provider disconnected before tool results"))?
            .map_err(io::Error::other)?;
        let value: Value = serde_json::from_str(message.to_text().map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        if let Some(responses) = value
            .pointer("/toolResponse/functionResponses")
            .and_then(Value::as_array)
        {
            response_ids.extend(responses.iter().filter_map(|response| {
                response
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }));
        }
    }
    response_ids.sort();
    if response_ids != ["native-7", "unknown-8"] {
        return Err(io::Error::other("tool result identities changed"));
    }
    socket.close(None).await.map_err(io::Error::other)
}

async fn check_outputs(outputs: &[LiveOutput], host: &Host) -> CheckResult<()> {
    for (index, output) in outputs.iter().enumerate() {
        if let LiveOutput::Json {
            frame:
                ServerFrame::Transcript {
                    role,
                    text,
                    is_final: true,
                },
        } = output
            && outputs.get(index + 1)
                != Some(&LiveOutput::Persist {
                    role: *role,
                    text: text.clone(),
                })
        {
            return Err(
                io::Error::other("final transcript was not followed by persistence").into(),
            );
        }
    }
    let calls = host.calls.lock().await;
    if calls.len() != 1
        || calls[0].call_id != "native-7"
        || calls[0].arguments != json!({"input": "safe"})
    {
        return Err(
            io::Error::other("unknown call executed or native call provenance changed").into(),
        );
    }
    for id in ["native-7", "unknown-8"] {
        if !outputs.iter().any(|output| matches!(output, LiveOutput::Json { frame: ServerFrame::ToolFinished { call_id, ok: false, cancelled: false, .. } } if call_id == id)) { return Err(io::Error::other("denied/unknown tool was fabricated as success").into()); }
    }
    Ok(())
}

#[derive(Clone, Default)]
struct ClientTransport {
    outputs: Arc<Mutex<Vec<LiveOutput>>>,
    committed: Arc<Mutex<std::collections::HashSet<String>>>,
    closed: Arc<tokio::sync::Notify>,
    attempts: Arc<Mutex<usize>>,
    closes: Arc<Mutex<usize>>,
}

#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.LiveTransport")]
impl ClientTransport {
    async fn read_live_input(
        &self,
        request: ReadLiveInputRequest,
    ) -> tinybus::Result<ReadLiveInputReply> {
        // The module reads this replayable mechanical socket callback itself;
        // the verification host never drives the provider or session pump.
        tokio::task::yield_now().await;
        Ok(ReadLiveInputReply {
            sequence: if request.acknowledged_frame == 0 {
                1
            } else {
                request.acknowledged_frame
            },
            frame: (request.acknowledged_frame == 0).then(|| TransportFrame::Text {
                text: r#"{"type":"text","text":"hello"}"#.into(),
            }),
        })
    }
    async fn deliver_live_effects(
        &self,
        request: DeliverLiveEffectsRequest,
    ) -> tinybus::Result<DeliverLiveEffectsReply> {
        let mut attempts = self.attempts.lock().await;
        *attempts += 1;
        if self.committed.lock().await.insert(request.delivery_id) {
            self.outputs.lock().await.extend(request.outputs);
        }
        if *attempts == 1 {
            return Err(tinybus::Error::failed(
                "verification lost reply after committing effects",
            ));
        }
        Ok(DeliverLiveEffectsReply { delivered: true })
    }
    async fn close_live_transport(
        &self,
        _request: CloseLiveTransportRequest,
    ) -> tinybus::Result<CloseLiveTransportReply> {
        *self.closes.lock().await += 1;
        self.closed.notify_one();
        Ok(CloseLiveTransportReply { closed: true })
    }
}

async fn verify_bound_transport(
    proxy: &tinybus::Proxy,
    client: &tinybus::Connection,
    host: &Host,
    mut request: LiveStartRequest,
) -> CheckResult<()> {
    host.calls.lock().await.clear();
    let transport = ClientTransport::default();
    client
        .serve_at("/test/LiveTransport".try_into()?, transport.clone())
        .await?;
    client.request_name("ai.test.LiveTransport").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let provider = tokio::spawn(scripted_provider(listener));
    let reserved: LiveStatus = proxy.call("LiveReserve", ()).await?;
    request.handle = reserved.handle.clone();
    request.connection = tinyvoice_bus::live::session::LiveConnection::GeminiRelay {
        ws_url: format!("ws://{address}"),
        session_id: "provider-session".into(),
        model: "fixture-model".into(),
    };
    let _: LiveStatus = proxy
        .call(
            "LiveBindTransport",
            (LiveBindTransportRequest {
                handle: reserved.handle.clone(),
                port: LiveTransportPort {
                    bus_name: "ai.test.LiveTransport".into(),
                    object_path: "/test/LiveTransport".into(),
                    context_token: "known-client".into(),
                },
            },),
        )
        .await?;
    let _: LiveStatus = proxy.call("LiveStart", (request,)).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        transport.closed.notified(),
    )
    .await?;
    provider.await??;
    if *transport.closes.lock().await != 1 {
        return Err(io::Error::other("socket was not closed once").into());
    }
    let outputs = transport.outputs.lock().await;
    check_outputs(&outputs, host).await?;
    if outputs.iter().filter(|effect| matches!(effect, LiveOutput::Persist { role: TranscriptRole::User, text } if text == "hello")).count() != 1 {
        return Err(io::Error::other("typed input persistence repeated or was lost").into());
    }
    if !outputs.iter().any(|effect| {
        matches!(
            effect,
            LiveOutput::Json {
                frame: ServerFrame::Closed { .. }
            }
        )
    }) {
        return Err(io::Error::other("module transport lost terminal output").into());
    }
    let _: LiveStatus = proxy
        .call(
            "LiveRelease",
            (LiveHandleRequest {
                handle: reserved.handle,
            },),
        )
        .await?;
    Ok(())
}
