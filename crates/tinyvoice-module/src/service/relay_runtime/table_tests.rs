//! Native reservation bounds and owned-worker cleanup fixtures.

use super::*;

fn limits(active: u16, pending: u16) -> RelayLimits {
    RelayLimits {
        max_active_turns: active,
        max_pending_turns: pending,
    }
}
fn request(handle: RelayHandle) -> RelayStartRequest {
    RelayStartRequest {
        handle,
        port: tinyvoice_bus::relay::runtime::RelayHostPort {
            bus_name: "ai.test.Relay".into(),
            object_path: "/test/Relay".into(),
            context_token: "known".into(),
        },
        acknowledgment_deadline_ms: 100,
        turn_timeout_ms: 1000,
        empty_reply_text: "pause".into(),
        deadline_reply_text: "working".into(),
        lost_reply_text: "lost".into(),
    }
}
async fn fixture() -> tinybus::Result<(Turns, tinybus::Connection)> {
    let bus = tinybus::transport::memory::MemoryBus::new();
    tinybus::broker::Broker::new().spawn(bus.clone());
    let connection = tinybus::Connection::connect(bus.connect().await?).await?;
    Ok((Turns::with_connection(&connection), connection))
}

#[tokio::test(start_paused = true)]
async fn reservations_require_valid_fixed_limits_and_expire_without_evicting_running_turns()
-> tinybus::Result<()> {
    let turns = Turns::default();
    assert!(turns.reserve().is_err());
    assert!(format!("{turns:?}").contains("closed"));
    for invalid in [limits(0, 1), limits(17, 17), limits(1, 257), limits(2, 1)] {
        assert!(turns.configure(invalid).is_err());
    }
    assert_eq!(turns.configure(limits(1, 2))?, limits(1, 2));
    assert_eq!(turns.configure(limits(1, 2))?, limits(1, 2));
    assert!(turns.configure(limits(2, 2)).is_err());
    let active = turns.reserve()?.handle;
    turns
        .lease(&active)?
        .phase
        .lock()
        .map_err(|_| unavailable())?
        .state = RelayState::Running;
    turns.reserve()?;
    assert!(turns.reserve().is_err());
    tokio::time::advance(Duration::from_secs(60)).await;
    turns.reserve()?;
    assert_eq!(
        turns.state.lock().map_err(|_| unavailable())?.leases.len(),
        2
    );
    assert!(turns.lease(&active).is_ok());
    turns.shutdown().await?;
    assert!(turns.configure(limits(1, 2)).is_err());
    assert!(turns.reserve().is_err());
    turns.shutdown().await
}

#[tokio::test(start_paused = true)]
async fn relay_start_rejects_bad_timing_payload_changed_or_expired_inputs_before_host_work()
-> tinybus::Result<()> {
    let (turns, _connection) = fixture().await?;
    turns.configure(limits(1, 16))?;
    let handle = turns.reserve()?.handle;
    let base = request(handle.clone());
    for bad in [
        RelayStartRequest {
            acknowledgment_deadline_ms: 0,
            ..base.clone()
        },
        RelayStartRequest {
            acknowledgment_deadline_ms: 60_001,
            ..base.clone()
        },
        RelayStartRequest {
            turn_timeout_ms: 1,
            ..base.clone()
        },
        RelayStartRequest {
            turn_timeout_ms: 1_800_001,
            ..base.clone()
        },
        RelayStartRequest {
            deadline_reply_text: "x".repeat(64 * 1024),
            ..base.clone()
        },
    ] {
        assert!(turns.start(bad).is_err());
    }
    let mut bad = base.clone();
    bad.port.object_path = "invalid path".into();
    assert!(turns.start(bad).is_err());
    assert_eq!(turns.start(base.clone())?.state, RelayState::Queued);
    assert_eq!(turns.start(base.clone())?.state, RelayState::Queued);
    let mut changed = base;
    changed.lost_reply_text = "changed".into();
    assert!(turns.start(changed).is_err());
    turns.lease(&handle)?.cancel();
    assert!(turns.release(handle.clone()).await.is_err());
    assert_eq!(turns.release(handle).await?.state, RelayState::Closed);
    let expired = turns.reserve()?.handle;
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(turns.start(request(expired.clone())).is_err());
    turns.release(expired).await?;
    let no_connection = Turns::default();
    no_connection.configure(limits(1, 1))?;
    let handle = no_connection.reserve()?.handle;
    assert!(no_connection.start(request(handle)).is_err());
    Ok(())
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn block(lease: &Lease, dropped: Arc<AtomicBool>) -> tinybus::Result<()> {
    lease.phase.lock().map_err(|_| unavailable())?.task = Some(tokio::spawn(async move {
        let _guard = Dropped(dropped);
        std::future::pending::<()>().await;
    }));
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn relay_shutdown_joins_all_stalled_workers_under_one_cleanup_deadline() -> tinybus::Result<()>
{
    let turns = Turns::default();
    turns.configure(limits(1, 16))?;
    let mut dropped = Vec::new();
    for _ in 0..3 {
        let handle = turns.reserve()?.handle;
        let guard = Arc::new(AtomicBool::new(false));
        block(turns.lease(&handle)?.as_ref(), guard.clone())?;
        dropped.push(guard);
    }
    tokio::task::yield_now().await;
    let before = tokio::time::Instant::now();
    assert!(turns.shutdown().await.is_err());
    assert_eq!(before.elapsed(), Duration::from_secs(2));
    assert!(dropped.iter().all(|guard| guard.load(Ordering::SeqCst)));
    turns.shutdown().await
}
#[tokio::test]
async fn dropped_release_future_and_scheduler_abort_their_owned_worker() -> tinybus::Result<()> {
    let turns = Arc::new(Turns::default());
    turns.configure(limits(1, 16))?;
    let handle = turns.reserve()?.handle;
    let lease = turns.lease(&handle)?;
    let dropped = Arc::new(AtomicBool::new(false));
    block(&lease, dropped.clone())?;
    tokio::task::yield_now().await;
    let release_turns = turns.clone();
    let release = tokio::spawn(async move { release_turns.release(handle).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !lease.cancelled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| tinybus::Error::failed("fixture cancel deadline"))?;
    release.abort();
    assert!(release.await.is_err());
    tokio::time::timeout(Duration::from_secs(1), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| tinybus::Error::failed("fixture worker drop deadline"))?;
    let handle = turns.reserve()?.handle;
    let dropped = Arc::new(AtomicBool::new(false));
    block(turns.lease(&handle)?.as_ref(), dropped.clone())?;
    tokio::task::yield_now().await;
    drop(turns);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| tinybus::Error::failed("fixture scheduler drop deadline"))?;
    Ok(())
}
