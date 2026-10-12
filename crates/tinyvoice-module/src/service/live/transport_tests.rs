//! Actual bus callbacks pin delivery retry, EOF and bounded socket cleanup.

use super::super::table::Sessions;
use super::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use tinybus::{broker::Broker, transport::memory::MemoryBus};
use tinyvoice_bus::live::{ServerFrame, TransportFrame, transport::LiveBindTransportRequest};

#[derive(Clone, Copy, Default)]
enum DeliveryMode {
    #[default]
    Accept,
    LoseFirstReply,
    Reject,
}

#[derive(Clone, Default)]
struct Socket {
    committed: Arc<tokio::sync::Mutex<HashSet<String>>>,
    effects: Arc<tokio::sync::Mutex<Vec<LiveOutput>>>,
    attempts: Arc<AtomicUsize>,
    polls: Arc<tokio::sync::Mutex<usize>>,
    closes: Arc<AtomicUsize>,
    closed: Arc<tokio::sync::Notify>,
    delivery: DeliveryMode,
    unresponsive_close: bool,
    invalid_sequence: bool,
}

#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.LiveTransport")]
impl Socket {
    async fn read_live_input(
        &self,
        request: ReadLiveInputRequest,
    ) -> tinybus::Result<ReadLiveInputReply> {
        *self.polls.lock().await += 1;
        Ok(ReadLiveInputReply {
            sequence: request.acknowledged_frame + if self.invalid_sequence { 2 } else { 1 },
            frame: Some(TransportFrame::Closed),
        })
    }
    async fn deliver_live_effects(
        &self,
        request: DeliverLiveEffectsRequest,
    ) -> tinybus::Result<DeliverLiveEffectsReply> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if matches!(self.delivery, DeliveryMode::Reject) {
            return Ok(DeliverLiveEffectsReply { delivered: false });
        }
        if self.committed.lock().await.insert(request.delivery_id) {
            self.effects.lock().await.extend(request.outputs);
        }
        if matches!(self.delivery, DeliveryMode::LoseFirstReply) && attempt == 0 {
            return Err(tinybus::Error::failed(
                "fixture lost acknowledgement after commit",
            ));
        }
        Ok(DeliverLiveEffectsReply { delivered: true })
    }
    async fn close_live_transport(
        &self,
        _request: CloseLiveTransportRequest,
    ) -> tinybus::Result<CloseLiveTransportReply> {
        if self.unresponsive_close {
            std::future::pending::<()>().await;
        }
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.closed.notify_one();
        Ok(CloseLiveTransportReply { closed: true })
    }
}

async fn fixture(
    socket: Socket,
) -> tinybus::Result<(
    Sessions,
    tinybus::Connection,
    tinybus::Connection,
    LiveHandle,
    Arc<Lease>,
)> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let host = tinybus::Connection::connect(bus.connect().await?).await?;
    host.serve_at("/test/Transport".try_into()?, socket).await?;
    host.request_name("ai.test.Transport").await?;
    let module = tinybus::Connection::connect(bus.connect().await?).await?;
    let sessions = Sessions::with_connection(&module);
    let handle = sessions.reserve()?.handle;
    let lease = sessions.lease(&handle)?;
    sessions.bind_transport(LiveBindTransportRequest {
        handle: handle.clone(),
        port: LiveTransportPort {
            bus_name: "ai.test.Transport".into(),
            object_path: "/test/Transport".into(),
            context_token: "known".into(),
        },
    })?;
    Ok((sessions, module, host, handle, lease))
}

#[tokio::test]
async fn lost_delivery_reply_reuses_identity_and_preserves_order_without_duplicate_persistence()
-> tinybus::Result<()> {
    let socket = Socket {
        delivery: DeliveryMode::LoseFirstReply,
        ..Socket::default()
    };
    let (sessions, _module, _host, handle, lease) = fixture(socket.clone()).await?;
    let effects = vec![
        LiveOutput::Json {
            frame: ServerFrame::Transcript {
                role: TranscriptRole::Agent,
                text: "complete".into(),
                is_final: true,
            },
        },
        LiveOutput::Persist {
            role: TranscriptRole::Agent,
            text: "complete".into(),
        },
    ];
    lease
        .outputs
        .send(effects.clone())
        .await
        .map_err(|_| tinybus::Error::failed("fixture queue"))?;
    lease
        .phase
        .lock()
        .map_err(|_| tinybus::Error::failed("fixture lock"))?
        .state = LiveState::Closed;
    tokio::time::timeout(Duration::from_secs(1), socket.closed.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture deadline"))?;
    assert_eq!(socket.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(*socket.effects.lock().await, effects);
    assert_eq!(socket.committed.lock().await.len(), 1);
    sessions.release(handle).await?;
    Ok(())
}

#[tokio::test]
async fn client_eof_stops_module_coordinator_and_closes_the_known_socket_once()
-> tinybus::Result<()> {
    let socket = Socket::default();
    let (sessions, _module, _host, handle, lease) = fixture(socket.clone()).await?;
    lease
        .phase
        .lock()
        .map_err(|_| tinybus::Error::failed("fixture lock"))?
        .state = LiveState::Running;
    tokio::time::timeout(Duration::from_secs(1), socket.closed.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture deadline"))?;
    assert!(lease.cancelled.load(Ordering::SeqCst));
    assert_eq!(socket.closes.load(Ordering::SeqCst), 1);
    sessions.release(handle.clone()).await?;
    sessions.release(handle).await?;
    assert_eq!(socket.closes.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn noncontiguous_socket_input_fails_closed_without_applying_effects() -> tinybus::Result<()> {
    let socket = Socket {
        invalid_sequence: true,
        ..Socket::default()
    };
    let (sessions, _module, _host, handle, lease) = fixture(socket.clone()).await?;
    lease
        .phase
        .lock()
        .map_err(|_| tinybus::Error::failed("fixture lock"))?
        .state = LiveState::Running;
    tokio::time::timeout(Duration::from_secs(1), socket.closed.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture deadline"))?;
    assert_eq!(socket.attempts.load(Ordering::SeqCst), 0);
    sessions.release(handle).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn unresponsive_socket_cleanup_is_bounded_and_never_acknowledged_as_success()
-> tinybus::Result<()> {
    let socket = Socket {
        unresponsive_close: true,
        ..Socket::default()
    };
    let (sessions, _module, _host, handle, lease) = fixture(socket.clone()).await?;
    lease
        .phase
        .lock()
        .map_err(|_| tinybus::Error::failed("fixture lock"))?
        .state = LiveState::Running;
    // Release cancels the active callback as well as the coordinator. The host
    // refuses cleanup forever; the module must return within its fixed bound.
    let before = tokio::time::Instant::now();
    assert!(sessions.release(handle.clone()).await.is_err());
    assert!(before.elapsed() <= Duration::from_secs(2));
    assert_eq!(socket.closes.load(Ordering::SeqCst), 0);
    assert!(
        !lease
            .phase
            .lock()
            .map_err(|_| tinybus::Error::failed("fixture lock"))?
            .transport_cleanup_confirmed
    );
    sessions.release(handle).await?;
    Ok(())
}

fn port() -> LiveTransportPort {
    LiveTransportPort {
        bus_name: "ai.test.Transport".into(),
        object_path: "/test/Transport".into(),
        context_token: "known".into(),
    }
}

#[tokio::test]
async fn portable_transport_controls_preserve_native_input_and_ignore_late_noncontrols()
-> tinybus::Result<()> {
    use tinyagents_live::tinyliveagents::{ClientCommand, LiveSession};
    let socket = Socket::default();
    let (sessions, module, _host, handle, lease) = fixture(socket).await?;
    let proxy = module.proxy("ai.test.Transport", "/test/Transport", INTERFACE)?;
    let (commands, mut receive) = tokio::sync::mpsc::channel(8);
    let (_events, events) = tokio::sync::mpsc::channel(8);
    let native = LiveSession::from_channels(commands, events);
    lease
        .phase
        .lock()
        .map_err(|_| tinybus::Error::failed("fixture lock"))?
        .sender = Some(native.sender());
    for input in [
        PreparedLiveInput::Control(ClientFrame::Interrupt),
        PreparedLiveInput::Control(ClientFrame::Stop),
        PreparedLiveInput::Transport(LiveTransportAction::Audio { pcm16: vec![1, 2] }),
    ] {
        assert!(apply_input(&lease, &handle, &port(), &proxy, 1, input).await?);
    }
    assert!(matches!(
        receive.recv().await,
        Some(ClientCommand::Interrupt)
    ));
    assert!(matches!(receive.recv().await, Some(ClientCommand::Close)));
    assert!(
        matches!(receive.recv().await,Some(ClientCommand::Audio(bytes)) if bytes.as_ref() == [1,2])
    );
    assert!(
        apply_input(
            &lease,
            &handle,
            &port(),
            &proxy,
            1,
            PreparedLiveInput::Transport(LiveTransportAction::Ignore)
        )
        .await?
    );
    assert!(
        apply_input(
            &lease,
            &handle,
            &port(),
            &proxy,
            1,
            PreparedLiveInput::Control(ClientFrame::Start {
                provider: None,
                thread_id: None,
                client_id: None,
                input_sample_rate: 16000
            })
        )
        .await?
    );
    for input in [
        PreparedLiveInput::Control(ClientFrame::Text {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
        }),
        PreparedLiveInput::Transport(LiveTransportAction::Audio {
            pcm16: vec![0; MAX_AUDIO_BYTES + 1],
        }),
    ] {
        assert!(
            apply_input(&lease, &handle, &port(), &proxy, 1, input)
                .await
                .is_err()
        );
    }
    sessions.release(handle).await?;
    Ok(())
}

#[tokio::test]
async fn rejected_delivery_retries_boundedly_without_persisting_or_dispatching_input()
-> tinybus::Result<()> {
    let socket = Socket {
        delivery: DeliveryMode::Reject,
        ..Socket::default()
    };
    let (sessions, module, _host, handle, lease) = fixture(socket.clone()).await?;
    let proxy = module.proxy("ai.test.Transport", "/test/Transport", INTERFACE)?;
    assert!(
        apply_input(
            &lease,
            &handle,
            &port(),
            &proxy,
            1,
            PreparedLiveInput::Control(ClientFrame::Text {
                text: "uncommitted".into()
            })
        )
        .await
        .is_err()
    );
    assert_eq!(socket.attempts.load(Ordering::SeqCst), 2);
    assert!(socket.effects.lock().await.is_empty());
    let outputs = vec![LiveOutput::Persist {
        role: TranscriptRole::User,
        text: "\0".repeat(2 * MAX_BATCH_BYTES),
    }];
    assert!(
        deliver(&proxy, &port(), &handle, "oversized".into(), outputs)
            .await
            .is_err()
    );
    assert_eq!(socket.attempts.load(Ordering::SeqCst), 2);
    sessions.release(handle).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn forgotten_bound_reservation_expires_and_closes_its_mechanical_socket()
-> tinybus::Result<()> {
    let socket = Socket::default();
    let (sessions, _module, _host, handle, lease) = fixture(socket.clone()).await?;
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::time::timeout(Duration::from_secs(1), socket.closed.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture deadline"))?;
    assert!(lease.cancelled.load(Ordering::SeqCst));
    sessions.release(handle).await?;
    Ok(())
}
