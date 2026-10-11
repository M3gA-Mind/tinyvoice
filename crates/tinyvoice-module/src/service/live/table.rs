//! Bounded leases, cancellation and replayable live event batches.

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tinyagents_live::tinyliveagents::LiveSender;
use tinyvoice_bus::live::session::{
    LiveBatch, LiveCommand, LiveHandle, LiveOutput, LiveReadRequest, LiveSendRequest,
    LiveStartRequest, LiveState, LiveStatus,
};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

const MAX_LEASES: usize = 16;
pub(super) const MAX_GROUPS: usize = 32;
pub(super) const MAX_BATCH_BYTES: usize = 1024 * 1024;
pub(super) const MAX_AUDIO_BYTES: usize = 64 * 1024;
pub(super) const MAX_TEXT_BYTES: usize = 64 * 1024;
const RESERVATION_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Mutable state never held across an await.
pub(super) struct Phase {
    pub(super) state: LiveState,
    pub(super) sender: Option<LiveSender>,
    pub(super) task: Option<JoinHandle<()>>,
    transport_task: Option<JoinHandle<()>>,
    transport: Option<tinyvoice_bus::live::transport::LiveTransportPort>,
    pub(super) transport_cleanup_confirmed: bool,
    request: Option<LiveStartRequest>,
    pub(super) cleanup_confirmed: bool,
}

struct Reader {
    rx: mpsc::Receiver<Vec<LiveOutput>>,
    carry: Option<Vec<LiveOutput>>,
    pending: Option<LiveBatch>,
    last_acknowledged: u64,
    next_batch: u64,
}

/// One owned native provider task and its bounded event queue.
pub(super) struct Lease {
    pub(super) phase: Mutex<Phase>,
    pub(super) outputs: mpsc::Sender<Vec<LiveOutput>>,
    pub(super) cancelled: AtomicBool,
    changed: Notify,
    reader: tokio::sync::Mutex<Reader>,
    created: tokio::time::Instant,
}

impl Lease {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::channel(MAX_GROUPS);
        Self {
            phase: Mutex::new(Phase {
                state: LiveState::Reserved,
                sender: None,
                task: None,
                transport_task: None,
                transport: None,
                transport_cleanup_confirmed: true,
                request: None,
                cleanup_confirmed: true,
            }),
            outputs: tx,
            cancelled: AtomicBool::new(false),
            changed: Notify::new(),
            reader: tokio::sync::Mutex::new(Reader {
                rx,
                carry: None,
                pending: None,
                last_acknowledged: 0,
                next_batch: 1,
            }),
            created: tokio::time::Instant::now(),
        }
    }

    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
    }

    pub(super) async fn wait_cancelled(&self) {
        let notified = self.changed.notified();
        tokio::pin!(notified);
        // Register before testing the sticky flag so cancellation cannot land
        // in the gap between the flag check and the await.
        notified.as_mut().enable();
        if !self.cancelled.load(Ordering::SeqCst) {
            notified.await;
        }
    }

    pub(super) fn status(&self, handle: LiveHandle) -> tinybus::Result<LiveStatus> {
        let phase = self.phase.lock().map_err(|_| unavailable())?;
        Ok(LiveStatus {
            handle,
            state: phase.state,
        })
    }
}

/// Admission table; shutdown permanently closes this module instance.
pub(in crate::service) struct Sessions {
    closed: AtomicBool,
    leases: Mutex<HashMap<LiveHandle, Arc<Lease>>>,
    connection: Mutex<Option<tinybus::WeakConnection>>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl Default for Sessions {
    fn default() -> Self {
        Self {
            closed: AtomicBool::new(false),
            leases: Mutex::new(HashMap::new()),
            connection: Mutex::new(None),
        }
    }
}

impl Sessions {
    pub(in crate::service) fn with_connection(connection: &tinybus::Connection) -> Self {
        Self {
            connection: Mutex::new(Some(connection.downgrade())),
            closed: AtomicBool::new(false),
            leases: Mutex::new(HashMap::new()),
        }
    }

    pub(in crate::service) fn reserve(&self) -> tinybus::Result<LiveStatus> {
        let mut leases = self.leases.lock().map_err(|_| unavailable())?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(tinybus::Error::failed("live module admission is closed"));
        }
        leases.retain(|_, lease| {
            lease.phase.lock().is_ok_and(|phase| {
                let keep =
                    phase.state != LiveState::Reserved || lease.created.elapsed() < RESERVATION_TTL;
                if !keep {
                    lease.cancel();
                }
                keep
            })
        });
        if leases.len() >= MAX_LEASES {
            return Err(tinybus::Error::failed("live session capacity is exhausted"));
        }
        loop {
            let id = random_id()?;
            let handle = LiveHandle { id };
            if !leases.contains_key(&handle) {
                leases.insert(handle.clone(), Arc::new(Lease::new()));
                return Ok(LiveStatus {
                    handle,
                    state: LiveState::Reserved,
                });
            }
        }
    }

    pub(super) fn lease(&self, handle: &LiveHandle) -> tinybus::Result<Arc<Lease>> {
        self.leases
            .lock()
            .map_err(|_| unavailable())?
            .get(handle)
            .cloned()
            .ok_or_else(|| tinybus::Error::failed("unknown live session handle"))
    }

    pub(in crate::service) fn start(
        &self,
        request: LiveStartRequest,
    ) -> tinybus::Result<LiveStatus> {
        if !super::super::relay::fits(&request, MAX_BATCH_BYTES) {
            return Err(tinybus::Error::failed(
                "live startup exceeds the serialized size limit",
            ));
        }
        let lease = self.lease(&request.handle)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .and_then(tinybus::WeakConnection::upgrade)
            .ok_or_else(|| tinybus::Error::failed("live callback connection is unavailable"))?;
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        if let Some(previous) = &phase.request {
            if previous == &request {
                return Ok(LiveStatus {
                    handle: request.handle,
                    state: phase.state,
                });
            }
            return Err(tinybus::Error::failed(
                "live session was already started with different inputs",
            ));
        }
        if self.closed.load(Ordering::SeqCst)
            || lease.cancelled.load(Ordering::SeqCst)
            || phase.state != LiveState::Reserved
            || lease.created.elapsed() >= RESERVATION_TTL
        {
            return Err(tinybus::Error::failed(
                "live reservation is closed or expired",
            ));
        }
        let prepared = super::prepare::prepare(&request, &connection)?;
        phase.state = LiveState::Starting;
        phase.cleanup_confirmed = false;
        phase.request = Some(request.clone());
        let task_lease = lease.clone();
        let handle = request.handle.clone();
        phase.task = Some(tokio::spawn(super::actor::run(
            task_lease, request, prepared, connection,
        )));
        Ok(LiveStatus {
            handle,
            state: LiveState::Starting,
        })
    }

    pub(in crate::service) async fn send(
        &self,
        request: LiveSendRequest,
    ) -> tinybus::Result<LiveStatus> {
        let lease = self.lease(&request.handle)?;
        ensure_manual_transport(&lease)?;
        send_lease(&lease, request).await
    }

    pub(in crate::service) async fn read(
        &self,
        request: LiveReadRequest,
    ) -> tinybus::Result<LiveBatch> {
        let lease = self.lease(&request.handle)?;
        ensure_manual_transport(&lease)?;
        read_lease(&lease, request).await
    }

    pub(in crate::service) fn bind_transport(
        &self,
        request: tinyvoice_bus::live::transport::LiveBindTransportRequest,
    ) -> tinybus::Result<LiveStatus> {
        if !super::super::relay::fits(&request.port, 8 * 1024) {
            return Err(tinybus::Error::failed(
                "live transport endpoint exceeds its size limit",
            ));
        }
        let lease = self.lease(&request.handle)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .and_then(tinybus::WeakConnection::upgrade)
            .ok_or_else(|| {
                tinybus::Error::failed("live transport callback connection is unavailable")
            })?;
        // Validate the endpoint grammar before claiming transport ownership.
        connection.proxy(
            request.port.bus_name.as_str(),
            request.port.object_path.as_str(),
            tinyvoice_bus::live::transport::INTERFACE,
        )?;
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        if let Some(port) = &phase.transport {
            return if port == &request.port {
                Ok(LiveStatus {
                    handle: request.handle,
                    state: phase.state,
                })
            } else {
                Err(tinybus::Error::failed(
                    "live transport was already bound to a different endpoint",
                ))
            };
        }
        if phase.state != LiveState::Reserved
            || lease.cancelled.load(Ordering::SeqCst)
            || self.closed.load(Ordering::SeqCst)
        {
            return Err(tinybus::Error::failed(
                "live transport must bind to an open reservation",
            ));
        }
        phase.transport = Some(request.port.clone());
        phase.transport_cleanup_confirmed = false;
        phase.transport_task = Some(tokio::spawn(super::transport::run(
            lease.clone(),
            request.handle.clone(),
            request.port,
            connection,
        )));
        Ok(LiveStatus {
            handle: request.handle,
            state: phase.state,
        })
    }

    pub(in crate::service) async fn release(
        &self,
        handle: LiveHandle,
    ) -> tinybus::Result<LiveStatus> {
        let lease = self
            .leases
            .lock()
            .map_err(|_| unavailable())?
            .remove(&handle);
        if let Some(lease) = lease {
            stop_lease(&lease).await?;
        }
        Ok(LiveStatus {
            handle,
            state: LiveState::Closed,
        })
    }

    pub(in crate::service) async fn shutdown(&self) -> tinybus::Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        let leases: Vec<_> = self
            .leases
            .lock()
            .map_err(|_| unavailable())?
            .drain()
            .map(|(_, lease)| lease)
            .collect();
        // Release the service's connection ownership while actors retain their
        // own callback proxies until bounded cleanup finishes.
        self.connection.lock().map_err(|_| unavailable())?.take();
        for lease in &leases {
            lease.cancel();
        }
        // Start every bounded cleanup together: the shutdown deadline must not
        // multiply by the number of admitted sessions. JoinSet also aborts its
        // cleanup futures if the caller cancels shutdown.
        let mut cleanups = tokio::task::JoinSet::new();
        for lease in leases {
            cleanups.spawn(async move { stop_lease(&lease).await });
        }
        let mut failure = None;
        while let Some(result) = cleanups.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failure = Some(error),
                Err(_) => {
                    failure = Some(tinybus::Error::failed(
                        "live cleanup task terminated unexpectedly",
                    ));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

async fn stop_lease(lease: &Lease) -> tinybus::Result<()> {
    lease.cancel();
    let (provider, transport) = {
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        phase.sender = None;
        phase.state = LiveState::Closed;
        (
            phase.task.take().map(OwnedTask),
            phase.transport_task.take().map(OwnedTask),
        )
    };
    // Own both abort guards before awaiting either cleanup future.
    let (provider, transport) = tokio::join!(wait_owned(provider), wait_owned(transport));
    provider?;
    transport?;
    let phase = lease.phase.lock().map_err(|_| unavailable())?;
    if !phase.cleanup_confirmed || !phase.transport_cleanup_confirmed {
        return Err(tinybus::Error::failed(
            "live native tasks stopped but host context cleanup was not acknowledged",
        ));
    }
    Ok(())
}

/// The transport can finish its provider without waiting on its own join handle.
pub(super) async fn stop_provider(lease: &Lease) -> tinybus::Result<()> {
    lease.cancel();
    let task = {
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        phase.sender = None;
        phase.state = LiveState::Closed;
        phase.task.take().map(OwnedTask)
    };
    wait_owned(task).await
}

async fn wait_owned(task: Option<OwnedTask>) -> tinybus::Result<()> {
    if let Some(mut owned) = task {
        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut owned.0).await {
            Err(_) => {
                owned.0.abort();
                let _ = (&mut owned.0).await;
                return Err(tinybus::Error::failed(
                    "live worker cleanup exceeded its deadline",
                ));
            }
            Ok(Err(_)) => {
                return Err(tinybus::Error::failed(
                    "live worker terminated unexpectedly",
                ));
            }
            Ok(Ok(())) => {}
        }
    }
    Ok(())
}

/// Abort even when the caller drops a cleanup future before its deadline.
struct OwnedTask(JoinHandle<()>);

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        if let Ok(leases) = self.leases.get_mut() {
            for lease in leases.values() {
                lease.cancel();
                if let Ok(mut phase) = lease.phase.lock()
                    && let Some(task) = phase.task.take()
                {
                    task.abort();
                }
                if let Ok(mut phase) = lease.phase.lock()
                    && let Some(task) = phase.transport_task.take()
                {
                    task.abort();
                }
            }
        }
    }
}

fn unavailable() -> tinybus::Error {
    tinybus::Error::failed("live session state is unavailable")
}

#[cfg(test)]
#[path = "table_tests.rs"]
pub(super) mod tests;

/// Generate unpredictable correlation identities without deriving host secrets.
pub(super) fn random_id() -> tinybus::Result<String> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| tinybus::Error::failed("live identity generation failed"))?;
    Ok(format!("{:032x}", u128::from_be_bytes(random)))
}

fn ensure_manual_transport(lease: &Lease) -> tinybus::Result<()> {
    if lease
        .phase
        .lock()
        .map_err(|_| unavailable())?
        .transport
        .is_some()
    {
        Err(tinybus::Error::failed(
            "bound live transport owns input and output",
        ))
    } else {
        Ok(())
    }
}

/// Native input admission shared by the bus adapter and owned transport pump.
pub(super) async fn send_lease(
    lease: &Lease,
    request: LiveSendRequest,
) -> tinybus::Result<LiveStatus> {
    use base64::Engine as _;
    let sender = {
        let phase = lease.phase.lock().map_err(|_| unavailable())?;
        phase
            .sender
            .clone()
            .ok_or_else(|| tinybus::Error::failed("live session is not running"))?
    };
    let command = match request.command {
        LiveCommand::Audio { pcm16_base64 } => {
            if pcm16_base64.len() > MAX_AUDIO_BYTES.div_ceil(3) * 4 {
                return Err(tinybus::Error::failed("live audio exceeds its input limit"));
            }
            let pcm = base64::engine::general_purpose::STANDARD
                .decode(pcm16_base64)
                .map_err(|_| tinybus::Error::failed("invalid live audio encoding"))?;
            if pcm.len() > MAX_AUDIO_BYTES || pcm.len() % 2 != 0 {
                return Err(tinybus::Error::failed(
                    "live PCM16 audio exceeds its limit or has a partial sample",
                ));
            }
            tinyagents_live::tinyliveagents::ClientCommand::Audio(pcm.into())
        }
        LiveCommand::Text { text } => {
            if text.len() > MAX_TEXT_BYTES {
                return Err(tinybus::Error::failed("live text exceeds its input limit"));
            }
            tinyagents_live::tinyliveagents::ClientCommand::Text(text)
        }
        LiveCommand::Interrupt => tinyagents_live::tinyliveagents::ClientCommand::Interrupt,
        LiveCommand::Stop => tinyagents_live::tinyliveagents::ClientCommand::Close,
    };
    tokio::time::timeout(std::time::Duration::from_secs(1), sender.send(command))
        .await
        .map_err(|_| tinybus::Error::failed("live command queue did not accept input"))?
        .map_err(|error| tinybus::Error::failed(error.to_string()))?;
    lease.status(request.handle)
}

/// Ordered replayable output shared by manual and module-owned delivery.
pub(super) async fn read_lease(
    lease: &Lease,
    request: LiveReadRequest,
) -> tinybus::Result<LiveBatch> {
    let mut reader = lease.reader.lock().await;
    if let Some(ack) = request.acknowledged_batch {
        if reader
            .pending
            .as_ref()
            .is_some_and(|batch| batch.batch == ack)
        {
            reader.pending = None;
            reader.last_acknowledged = ack;
        } else if ack != reader.last_acknowledged {
            return Err(tinybus::Error::failed(
                "live batch acknowledgement is stale or unknown",
            ));
        }
    }
    if let Some(batch) = &reader.pending {
        return Ok(batch.clone());
    }
    let mut outputs = Vec::new();
    for _ in 0..MAX_GROUPS {
        let group = reader.carry.take().or_else(|| reader.rx.try_recv().ok());
        let Some(group) = group else { break };
        let before = outputs.len();
        outputs.extend(group);
        if !super::super::relay::fits(&outputs, MAX_BATCH_BYTES - 4096) {
            reader.carry = Some(outputs.split_off(before));
            if outputs.is_empty() {
                return Err(tinybus::Error::failed(
                    "live event group exceeds its output limit",
                ));
            }
            break;
        }
    }
    let state = lease.status(request.handle.clone())?.state;
    if outputs.is_empty() {
        return Ok(LiveBatch {
            handle: request.handle,
            batch: reader.last_acknowledged,
            state,
            outputs,
        });
    }
    let batch = LiveBatch {
        handle: request.handle,
        batch: reader.next_batch,
        state,
        outputs,
    };
    reader.next_batch = reader
        .next_batch
        .checked_add(1)
        .ok_or_else(|| tinybus::Error::failed("live batch sequence is exhausted"))?;
    reader.pending = Some(batch.clone());
    Ok(batch)
}

/// Bound reservations cannot retain an idle client forever without startup.
pub(super) fn reservation_expired(lease: &Lease) -> bool {
    lease.created.elapsed() >= RESERVATION_TTL
}
