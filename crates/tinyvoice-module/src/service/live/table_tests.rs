use super::*;

fn read_request(handle: &LiveHandle, ack: Option<u64>) -> LiveReadRequest {
    LiveReadRequest {
        handle: handle.clone(),
        acknowledged_batch: ack,
    }
}

#[tokio::test(start_paused = true)]
async fn abandoned_reservations_expire_without_evicting_running_sessions() -> tinybus::Result<()> {
    let sessions = Sessions::default();
    let active = sessions.reserve()?.handle;
    sessions
        .lease(&active)?
        .phase
        .lock()
        .map_err(|_| unavailable())?
        .state = LiveState::Running;
    for _ in 1..MAX_LEASES {
        sessions.reserve()?;
    }
    assert!(sessions.reserve().is_err());
    tokio::time::advance(RESERVATION_TTL).await;
    sessions.reserve()?;
    assert_eq!(
        sessions.lease(&active)?.status(active)?.state,
        LiveState::Running
    );
    assert_eq!(sessions.leases.lock().map_err(|_| unavailable())?.len(), 2);
    sessions.shutdown().await
}

#[tokio::test]
async fn unacknowledged_batches_replay_without_repeating_committed_persistence()
-> tinybus::Result<()> {
    let sessions = Sessions::default();
    let handle = sessions.reserve()?.handle;
    let lease = sessions.lease(&handle)?;
    let group = vec![LiveOutput::Persist {
        role: tinyvoice_bus::live::TranscriptRole::User,
        text: "first".into(),
    }];
    lease
        .outputs
        .send(group.clone())
        .await
        .map_err(|_| unavailable())?;
    let first = sessions.read(read_request(&handle, None)).await?;
    lease
        .outputs
        .send(vec![LiveOutput::Persist {
            role: tinyvoice_bus::live::TranscriptRole::Agent,
            text: "second".into(),
        }])
        .await
        .map_err(|_| unavailable())?;
    assert_eq!(first.outputs, group);
    assert_eq!(sessions.read(read_request(&handle, None)).await?, first);
    assert!(
        sessions
            .read(read_request(&handle, Some(first.batch + 1)))
            .await
            .is_err()
    );
    let second = sessions
        .read(read_request(&handle, Some(first.batch)))
        .await?;
    assert_eq!(second.batch, first.batch + 1);
    assert_eq!(
        sessions
            .read(read_request(&handle, Some(first.batch)))
            .await?,
        second
    );
    let empty = sessions
        .read(read_request(&handle, Some(second.batch)))
        .await?;
    assert!(empty.outputs.is_empty());
    assert_eq!(empty.batch, second.batch);
    sessions.shutdown().await
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn blocked_task(lease: &Lease, dropped: Arc<AtomicBool>) -> tinybus::Result<()> {
    lease.phase.lock().map_err(|_| unavailable())?.task = Some(tokio::spawn(async move {
        let _guard = Dropped(dropped);
        std::future::pending::<()>().await;
    }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn unresponsive_cleanup_is_aborted_and_release_retries_do_not_restart_it()
-> tinybus::Result<()> {
    let sessions = Sessions::default();
    let handle = sessions.reserve()?.handle;
    let lease = sessions.lease(&handle)?;
    let dropped = Arc::new(AtomicBool::new(false));
    blocked_task(&lease, dropped.clone())?;
    tokio::task::yield_now().await;
    let before = tokio::time::Instant::now();
    assert!(sessions.release(handle.clone()).await.is_err());
    assert_eq!(before.elapsed(), std::time::Duration::from_secs(2));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(lease.cancelled.load(Ordering::SeqCst));
    assert_eq!(
        sessions.release(handle.clone()).await?.state,
        LiveState::Closed
    );
    assert!(sessions.lease(&handle).is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn shutdown_has_one_cleanup_deadline_for_all_unresponsive_sessions() -> tinybus::Result<()> {
    let sessions = Sessions::default();
    let mut guards = Vec::new();
    for _ in 0..3 {
        let handle = sessions.reserve()?.handle;
        let dropped = Arc::new(AtomicBool::new(false));
        let lease = sessions.lease(&handle)?;
        blocked_task(&lease, dropped.clone())?;
        guards.push(dropped);
    }
    tokio::task::yield_now().await;
    let before = tokio::time::Instant::now();
    assert!(sessions.shutdown().await.is_err());
    assert_eq!(before.elapsed(), std::time::Duration::from_secs(2));
    assert!(guards.iter().all(|guard| guard.load(Ordering::SeqCst)));
    assert!(sessions.reserve().is_err());
    sessions.shutdown().await
}

#[tokio::test]
async fn dropping_module_aborts_owned_workers_without_waiting_for_host_cleanup()
-> tinybus::Result<()> {
    let sessions = Sessions::default();
    let handle = sessions.reserve()?.handle;
    let dropped = Arc::new(AtomicBool::new(false));
    let lease = sessions.lease(&handle)?;
    blocked_task(&lease, dropped.clone())?;
    tokio::task::yield_now().await;
    drop(sessions);
    tokio::task::yield_now().await;
    assert!(dropped.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancelling_release_does_not_detach_the_owned_provider_worker() -> tinybus::Result<()> {
    let sessions = Arc::new(Sessions::default());
    let handle = sessions.reserve()?.handle;
    let lease = sessions.lease(&handle)?;
    let dropped = Arc::new(AtomicBool::new(false));
    blocked_task(&lease, dropped.clone())?;
    tokio::task::yield_now().await;
    let releasing = tokio::spawn(async move { sessions.release(handle).await });
    tokio::task::yield_now().await;
    assert!(
        lease
            .phase
            .lock()
            .map_err(|_| unavailable())?
            .task
            .is_none()
    );
    releasing.abort();
    let _ = releasing.await;
    tokio::task::yield_now().await;
    assert!(dropped.load(Ordering::SeqCst));
    Ok(())
}
