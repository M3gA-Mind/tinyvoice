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
    fn new() -> Self {
        let (tx, rx) = mpsc::channel(MAX_GROUPS);
        Self {
            phase: Mutex::new(Phase {
                state: LiveState::Reserved,
                sender: None,
                task: None,
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
        self.changed.notify_one();
    }

    pub(super) async fn wait_cancelled(&self) {
        if !self.cancelled.load(Ordering::SeqCst) {
            self.changed.notified().await;
        }
    }

    fn status(&self, handle: LiveHandle) -> tinybus::Result<LiveStatus> {
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
    connection: Mutex<Option<tinybus::Connection>>,
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
    pub(in crate::service) fn with_connection(connection: tinybus::Connection) -> Self {
        Self {
            connection: Mutex::new(Some(connection)),
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
                phase.state != LiveState::Reserved || lease.created.elapsed() < RESERVATION_TTL
            })
        });
        if leases.len() >= MAX_LEASES {
            return Err(tinybus::Error::failed("live session capacity is exhausted"));
        }
        loop {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| tinybus::Error::failed("live handle generation failed"))?;
            let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
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

    fn lease(&self, handle: &LiveHandle) -> tinybus::Result<Arc<Lease>> {
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
            .clone()
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
        use base64::Engine as _;
        let lease = self.lease(&request.handle)?;
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

    pub(in crate::service) async fn read(
        &self,
        request: LiveReadRequest,
    ) -> tinybus::Result<LiveBatch> {
        let lease = self.lease(&request.handle)?;
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
        let mut failure = None;
        for lease in leases {
            if let Err(error) = stop_lease(&lease).await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

async fn stop_lease(lease: &Lease) -> tinybus::Result<()> {
    lease.cancel();
    let task = {
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        phase.sender = None;
        phase.state = LiveState::Closed;
        phase.task.take()
    };
    if let Some(mut task) = task {
        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await {
            Err(_) => {
                task.abort();
                let _ = task.await;
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
        if !lease
            .phase
            .lock()
            .map_err(|_| unavailable())?
            .cleanup_confirmed
        {
            return Err(tinybus::Error::failed(
                "live provider stopped but host context cleanup was not acknowledged",
            ));
        }
    }
    Ok(())
}

impl Drop for Sessions {
    fn drop(&mut self) {
        if let Ok(leases) = self.leases.get_mut() {
            for lease in leases.values() {
                lease.cancel();
                if let Ok(mut phase) = lease.phase.lock() {
                    if let Some(task) = phase.task.take() {
                        task.abort();
                    }
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
mod tests;
