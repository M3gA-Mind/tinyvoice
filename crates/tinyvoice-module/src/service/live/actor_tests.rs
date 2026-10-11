//! Bounded provider text and host cleanup regressions.

use super::*;

#[test]
fn oversized_provider_errors_are_rejected_before_event_projection() {
    for error in [
        Error::InvalidConfig("x".repeat(MAX_TEXT_BYTES + 1)),
        Error::Connect("x".repeat(MAX_TEXT_BYTES + 1)),
    ] {
        assert!(oversized_error(&error));
    }
    assert!(!oversized_error(&Error::Timeout));
    let error = Error::Provider("x".repeat(MAX_TEXT_BYTES + 1));
    assert!(oversized(&LiveAgentEvent::Live(LiveEvent::Error {
        error: error.clone(),
        fatal: false
    })));
    assert!(oversized(&LiveAgentEvent::Live(LiveEvent::Closed(
        CloseReason::Error(error)
    ))));
    assert!(oversized(&LiveAgentEvent::Live(LiveEvent::Closed(
        CloseReason::Remote {
            code: Some(1000),
            reason: "x".repeat(MAX_TEXT_BYTES + 1)
        }
    ))));
}

use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;
use tinyvoice_bus::live::session::LiveToolPort;

#[derive(Clone, Default)]
struct HostCleanup {
    finishes: Arc<AtomicUsize>,
    known_finished: Arc<Mutex<bool>>,
    unresponsive: bool,
}

#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.LiveTools")]
impl HostCleanup {
    async fn finish_live_context(
        &self,
        request: FinishLiveContextRequest,
    ) -> tinybus::Result<FinishLiveContextReply> {
        if request.context_token != "known-context" {
            return Ok(FinishLiveContextReply { finished: false });
        }
        if self.unresponsive {
            std::future::pending::<()>().await;
        }
        let mut finished = self
            .known_finished
            .lock()
            .map_err(|_| tinybus::Error::failed("fixture lock"))?;
        if !*finished {
            *finished = true;
            self.finishes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(FinishLiveContextReply { finished: true })
    }
}

async fn cleanup_fixture(
    unresponsive: bool,
) -> tinybus::Result<(
    tinybus::Connection,
    tinybus::Connection,
    LiveToolPort,
    HostCleanup,
)> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let host = tinybus::Connection::connect(bus.connect().await?).await?;
    let cleanup = HostCleanup {
        unresponsive,
        ..HostCleanup::default()
    };
    host.serve_at("/test/LiveTools".try_into()?, cleanup.clone())
        .await?;
    host.request_name("ai.test.LiveTools").await?;
    let module = tinybus::Connection::connect(bus.connect().await?).await?;
    let port = LiveToolPort {
        bus_name: "ai.test.LiveTools".into(),
        object_path: "/test/LiveTools".into(),
        context_token: "known-context".into(),
    };
    Ok((module, host, port, cleanup))
}

#[tokio::test(start_paused = true)]
async fn unresponsive_host_cleanup_expires_without_acknowledging_success() -> tinybus::Result<()> {
    let (module, _host, port, cleanup) = cleanup_fixture(true).await?;
    let before = tokio::time::Instant::now();
    assert!(!finish_context(&module, &port).await);
    assert_eq!(before.elapsed(), FINISH_TIMEOUT);
    assert_eq!(cleanup.finishes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn repeated_and_late_finish_only_revoke_known_host_context_once() -> tinybus::Result<()> {
    let (module, _host, mut port, cleanup) = cleanup_fixture(false).await?;
    assert!(finish_context(&module, &port).await);
    assert!(finish_context(&module, &port).await);
    assert_eq!(cleanup.finishes.load(Ordering::SeqCst), 1);
    port.context_token = "unknown-late-token".into();
    assert!(!finish_context(&module, &port).await);
    assert_eq!(cleanup.finishes.load(Ordering::SeqCst), 1);
    Ok(())
}

#[derive(Debug)]
struct FailingProvider(tinyagents_live::tinyliveagents::Capabilities);

#[async_trait::async_trait]
impl tinyagents_live::tinyliveagents::LiveProvider for FailingProvider {
    fn id(&self) -> &'static str {
        "fixture"
    }
    fn capabilities(&self) -> tinyagents_live::tinyliveagents::Capabilities {
        self.0
    }
    async fn connect(
        &self,
        _config: tinyagents_live::tinyliveagents::LiveConfig,
    ) -> tinyagents_live::tinyliveagents::Result<tinyagents_live::tinyliveagents::LiveSession> {
        Err(Error::Provider("\0".repeat(MAX_TEXT_BYTES + 1)))
    }
}

#[tokio::test]
async fn oversized_provider_startup_failure_becomes_a_bounded_terminal_reply() -> anyhow::Result<()>
{
    let (module, _host, port, cleanup) = cleanup_fixture(false).await?;
    let mut request: LiveStartRequest = serde_json::from_str(include_str!("fixtures/start.json"))?;
    request.tool_port = port;
    let mut prepared = super::super::prepare::prepare(&request, &module)?;
    prepared.provider = Box::new(FailingProvider(prepared.provider.capabilities()));
    let lease = Arc::new(Lease::new());
    run(lease.clone(), request.clone(), prepared, module).await;
    let batch = super::super::table::read_lease(
        &lease,
        tinyvoice_bus::live::session::LiveReadRequest {
            handle: request.handle,
            acknowledged_batch: None,
        },
    )
    .await?;
    assert!(
        matches!(batch.outputs.as_slice(), [LiveOutput::Json { frame: ServerFrame::Error { code, .. } }, LiveOutput::Json { frame: ServerFrame::Closed { .. } }] if code == "invalid_response")
    );
    assert!(super::super::super::relay::fits(&batch, MAX_TEXT_BYTES));
    assert_eq!(cleanup.finishes.load(Ordering::SeqCst), 1);
    Ok(())
}

#[derive(Debug)]
struct AudioProvider(tinyagents_live::tinyliveagents::Capabilities);

#[async_trait::async_trait]
impl tinyagents_live::tinyliveagents::LiveProvider for AudioProvider {
    fn id(&self) -> &'static str {
        "fixture-audio"
    }
    fn capabilities(&self) -> tinyagents_live::tinyliveagents::Capabilities {
        self.0
    }
    async fn connect(
        &self,
        _config: tinyagents_live::tinyliveagents::LiveConfig,
    ) -> tinyagents_live::tinyliveagents::Result<tinyagents_live::tinyliveagents::LiveSession> {
        let (commands, _commands) = tokio::sync::mpsc::channel(8);
        let (events, receive) = tokio::sync::mpsc::channel(8);
        events
            .send(LiveEvent::Audio(vec![0_u8; MAX_AUDIO_BYTES * 2 + 2].into()))
            .await
            .map_err(|_| Error::Closed)?;
        events
            .send(LiveEvent::OutputTranscript {
                text: "x".repeat(MAX_TEXT_BYTES + 1),
                is_final: true,
            })
            .await
            .map_err(|_| Error::Closed)?;
        Ok(tinyagents_live::tinyliveagents::LiveSession::from_channels(
            commands, receive,
        ))
    }
}

#[tokio::test]
async fn native_audio_is_chunked_before_an_oversized_stream_event_closes_the_session()
-> anyhow::Result<()> {
    let (module, _host, port, _cleanup) = cleanup_fixture(false).await?;
    let mut request: LiveStartRequest = serde_json::from_str(include_str!("fixtures/start.json"))?;
    request.tool_port = port;
    let mut prepared = super::super::prepare::prepare(&request, &module)?;
    prepared.provider = Box::new(AudioProvider(prepared.provider.capabilities()));
    let lease = Arc::new(Lease::new());
    run(lease.clone(), request.clone(), prepared, module).await;
    let batch = super::super::table::read_lease(
        &lease,
        tinyvoice_bus::live::session::LiveReadRequest {
            handle: request.handle,
            acknowledged_batch: None,
        },
    )
    .await?;
    let sizes = batch
        .outputs
        .iter()
        .filter_map(|output| match output {
            LiveOutput::Audio { pcm16_base64 } => {
                Some(BASE64.decode(pcm16_base64).map(|audio| audio.len()))
            }
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(sizes, [MAX_AUDIO_BYTES, MAX_AUDIO_BYTES, 2]);
    assert!(matches!(
        batch.outputs.last(),
        Some(LiveOutput::Json {
            frame: ServerFrame::Closed { .. }
        })
    ));
    Ok(())
}

#[tokio::test]
async fn cancellation_before_connect_still_finishes_the_known_host_context() -> anyhow::Result<()> {
    let (module, _host, port, cleanup) = cleanup_fixture(false).await?;
    let mut request: LiveStartRequest = serde_json::from_str(include_str!("fixtures/start.json"))?;
    request.tool_port = port;
    let prepared = super::super::prepare::prepare(&request, &module)?;
    let lease = Arc::new(Lease::new());
    lease.cancel();
    run(lease.clone(), request, prepared, module).await;
    assert_eq!(cleanup.finishes.load(Ordering::SeqCst), 1);
    assert!(
        lease
            .phase
            .lock()
            .map_err(|_| anyhow::anyhow!("fixture lock"))?
            .cleanup_confirmed
    );
    Ok(())
}

#[tokio::test]
async fn malformed_finish_endpoint_cannot_acknowledge_host_cleanup() -> tinybus::Result<()> {
    let (module, _host, mut port, cleanup) = cleanup_fixture(false).await?;
    port.object_path = "invalid path".into();
    assert!(!finish_context(&module, &port).await);
    assert_eq!(cleanup.finishes.load(Ordering::SeqCst), 0);
    Ok(())
}
