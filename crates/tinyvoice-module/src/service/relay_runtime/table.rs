//! Bounded relay reservations and owned worker teardown.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tinyvoice_bus::relay::runtime::{
    RelayHandle, RelayLimits, RelayStartRequest, RelayState, RelayStatus,
};
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinHandle,
};

pub(super) struct Phase {
    pub(super) state: RelayState,
    request: Option<RelayStartRequest>,
    task: Option<JoinHandle<()>>,
    pub(super) cleanup_confirmed: bool,
}
pub(super) struct Lease {
    pub(super) phase: Mutex<Phase>,
    cancelled: AtomicBool,
    changed: Notify,
    created: tokio::time::Instant,
}
impl Lease {
    fn new() -> Self {
        Self {
            phase: Mutex::new(Phase {
                state: RelayState::Reserved,
                request: None,
                task: None,
                cleanup_confirmed: true,
            }),
            cancelled: AtomicBool::new(false),
            changed: Notify::new(),
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
        notified.as_mut().enable();
        if !self.cancelled.load(Ordering::SeqCst) {
            notified.await;
        }
    }
}
struct Quota {
    limits: RelayLimits,
    slots: Arc<Semaphore>,
}
#[derive(Default)]
struct State {
    quota: Option<Arc<Quota>>,
    leases: HashMap<RelayHandle, Arc<Lease>>,
}

/// One configured relay scheduler, independent of host model and correlation policy.
#[derive(Default)]
pub(in crate::service) struct Turns {
    closed: AtomicBool,
    state: Mutex<State>,
    connection: Option<tinybus::WeakConnection>,
}
impl std::fmt::Debug for Turns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Turns")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}
impl Turns {
    pub(in crate::service) fn with_connection(connection: &tinybus::Connection) -> Self {
        Self {
            connection: Some(connection.downgrade()),
            closed: AtomicBool::new(false),
            state: Mutex::new(State::default()),
        }
    }
    pub(in crate::service) fn configure(
        &self,
        limits: RelayLimits,
    ) -> tinybus::Result<RelayLimits> {
        if limits.max_active_turns == 0
            || limits.max_active_turns > 16
            || limits.max_pending_turns < limits.max_active_turns
            || limits.max_pending_turns > 256
        {
            return Err(tinybus::Error::failed(
                "relay scheduling bounds are invalid",
            ));
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(tinybus::Error::failed("relay admission is closed"));
        }
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if let Some(quota) = &state.quota {
            return if quota.limits == limits {
                Ok(limits)
            } else {
                Err(tinybus::Error::failed(
                    "relay scheduling bounds were already configured",
                ))
            };
        }
        state.quota = Some(Arc::new(Quota {
            limits,
            slots: Arc::new(Semaphore::new(usize::from(limits.max_active_turns))),
        }));
        Ok(limits)
    }
    pub(in crate::service) fn reserve(&self) -> tinybus::Result<RelayStatus> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(tinybus::Error::failed("relay admission is closed"));
        }
        let limit = state
            .quota
            .as_ref()
            .ok_or_else(|| {
                tinybus::Error::failed("relay scheduling must be configured before reservation")
            })?
            .limits
            .max_pending_turns;
        state.leases.retain(|_, lease| {
            lease.phase.lock().is_ok_and(|phase| {
                let keep = phase.state != RelayState::Reserved
                    || lease.created.elapsed() < Duration::from_mins(1);
                if !keep {
                    lease.cancel();
                }
                keep
            })
        });
        if state.leases.len() >= usize::from(limit) {
            return Err(tinybus::Error::failed(
                "relay reservation capacity is exhausted",
            ));
        }
        loop {
            let handle = RelayHandle {
                id: super::super::live::random_id()?,
            };
            if let std::collections::hash_map::Entry::Vacant(entry) =
                state.leases.entry(handle.clone())
            {
                entry.insert(Arc::new(Lease::new()));
                return Ok(RelayStatus {
                    handle,
                    state: RelayState::Reserved,
                });
            }
        }
    }
    fn lease(&self, handle: &RelayHandle) -> tinybus::Result<Arc<Lease>> {
        self.state
            .lock()
            .map_err(|_| unavailable())?
            .leases
            .get(handle)
            .cloned()
            .ok_or_else(|| tinybus::Error::failed("unknown relay turn handle"))
    }
    pub(in crate::service) fn start(
        &self,
        request: RelayStartRequest,
    ) -> tinybus::Result<RelayStatus> {
        if !super::super::relay::fits(&request, 64 * 1024)
            || request.acknowledgment_deadline_ms == 0
            || request.acknowledgment_deadline_ms > 60_000
            || request.turn_timeout_ms < request.acknowledgment_deadline_ms
            || request.turn_timeout_ms > 1_800_000
        {
            return Err(tinybus::Error::failed(
                "relay startup timing or payload bounds are invalid",
            ));
        }
        let connection = self
            .connection
            .as_ref()
            .and_then(tinybus::WeakConnection::upgrade)
            .ok_or_else(|| tinybus::Error::failed("relay callback connection is unavailable"))?;
        connection.proxy(
            request.port.bus_name.as_str(),
            request.port.object_path.as_str(),
            tinyvoice_bus::relay::runtime::INTERFACE,
        )?;
        let lease = self.lease(&request.handle)?;
        let slots = self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .quota
            .as_ref()
            .ok_or_else(unavailable)?
            .slots
            .clone();
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        if let Some(previous) = &phase.request {
            return if previous == &request {
                Ok(RelayStatus {
                    handle: request.handle,
                    state: phase.state,
                })
            } else {
                Err(tinybus::Error::failed(
                    "relay turn was already started with different inputs",
                ))
            };
        }
        if self.closed.load(Ordering::SeqCst)
            || lease.cancelled.load(Ordering::SeqCst)
            || phase.state != RelayState::Reserved
            || lease.created.elapsed() >= Duration::from_mins(1)
        {
            return Err(tinybus::Error::failed(
                "relay reservation is closed or expired",
            ));
        }
        phase.state = RelayState::Queued;
        phase.cleanup_confirmed = false;
        phase.request = Some(request.clone());
        let handle = request.handle.clone();
        phase.task = Some(tokio::spawn(super::actor::run(
            lease.clone(),
            request,
            slots,
            connection,
        )));
        Ok(RelayStatus {
            handle,
            state: RelayState::Queued,
        })
    }
    pub(in crate::service) async fn release(
        &self,
        handle: RelayHandle,
    ) -> tinybus::Result<RelayStatus> {
        let lease = self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .leases
            .remove(&handle);
        if let Some(lease) = lease {
            stop(&lease).await?;
        }
        Ok(RelayStatus {
            handle,
            state: RelayState::Closed,
        })
    }
    pub(in crate::service) async fn shutdown(&self) -> tinybus::Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        let leases = {
            let mut state = self.state.lock().map_err(|_| unavailable())?;
            if let Some(quota) = &state.quota {
                quota.slots.close();
            }
            state
                .leases
                .drain()
                .map(|(_, lease)| lease)
                .collect::<Vec<_>>()
        };
        let mut pending = tokio::task::JoinSet::new();
        for lease in leases {
            lease.cancel();
            pending.spawn(async move { stop(&lease).await });
        }
        let mut failure = None;
        while let Some(result) = pending.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failure = Some(error),
                Err(_) => failure = Some(unavailable()),
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
struct OwnedTask(JoinHandle<()>);
impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn stop(lease: &Lease) -> tinybus::Result<()> {
    lease.cancel();
    let task = {
        let mut phase = lease.phase.lock().map_err(|_| unavailable())?;
        phase.state = RelayState::Closed;
        phase.task.take().map(OwnedTask)
    };
    if let Some(mut task) = task {
        match tokio::time::timeout(Duration::from_secs(2), &mut task.0).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(unavailable()),
            Err(_) => {
                task.0.abort();
                let _ = (&mut task.0).await;
                return Err(tinybus::Error::failed(
                    "relay cleanup exceeded its deadline",
                ));
            }
        }
    }
    if !lease
        .phase
        .lock()
        .map_err(|_| unavailable())?
        .cleanup_confirmed
    {
        return Err(tinybus::Error::failed(
            "relay host cleanup was not acknowledged",
        ));
    }
    Ok(())
}
impl Drop for Turns {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            for lease in state.leases.values() {
                lease.cancel();
                if let Ok(mut phase) = lease.phase.lock()
                    && let Some(task) = phase.task.take()
                {
                    task.abort();
                }
            }
        }
    }
}
fn unavailable() -> tinybus::Error {
    tinybus::Error::failed("relay state is unavailable")
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
