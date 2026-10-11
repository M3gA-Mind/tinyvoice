//! Actual bus callbacks pin delivery retry, EOF and bounded socket cleanup.

use super::super::table::Sessions;
use super::*;
use std::collections::HashSet;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tinybus::{broker::Broker, transport::memory::MemoryBus};
use tinyvoice_bus::live::{ServerFrame, TransportFrame, transport::LiveBindTransportRequest};

#[derive(Clone, Default)]
struct Socket {
    committed: Arc<Mutex<HashSet<String>>>,
    effects: Arc<Mutex<Vec<LiveOutput>>>,
    attempts: Arc<AtomicUsize>,
    polls: Arc<tokio::sync::Mutex<usize>>,
    closes: Arc<AtomicUsize>,
    closed: Arc<tokio::sync::Notify>,
    first_reply_lost: bool,
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
        if self
            .committed
            .lock()
            .map_err(|_| tinybus::Error::failed("fixture lock"))?
            .insert(request.delivery_id)
        {
            self.effects
                .lock()
                .map_err(|_| tinybus::Error::failed("fixture lock"))?
                .extend(request.outputs);
        }
        if self.first_reply_lost && attempt == 0 {
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
        first_reply_lost: true,
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
    assert_eq!(
        *socket
            .effects
            .lock()
            .map_err(|_| tinybus::Error::failed("fixture lock"))?,
        effects
    );
    assert_eq!(
        socket
            .committed
            .lock()
            .map_err(|_| tinybus::Error::failed("fixture lock"))?
            .len(),
        1
    );
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
