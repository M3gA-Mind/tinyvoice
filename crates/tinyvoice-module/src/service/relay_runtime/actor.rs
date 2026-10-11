//! Deadline coordination over retained selected progress and host-owned native work.

use super::table::Lease;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tinyvoice_bus::relay::runtime::{
    APPLY_METHOD, ApplyRelayEffectsReply, ApplyRelayEffectsRequest, FINISH_METHOD,
    FinishRelayBrainReply, INTERFACE, READ_METHOD, ReadRelayBrainReply, ReadRelayBrainRequest,
    RelayBrainEvent, RelayEffect, RelayInvocationRequest, RelayOutcome, RelayStartRequest,
    RelayState, START_METHOD, StartRelayBrainReply,
};

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_TEXT: usize = 256 * 1024;
#[derive(Default)]
struct Delivery {
    sequence: u64,
    unacknowledged: bool,
    expiry: Option<tokio::time::Instant>,
}

type Incoming = Pin<Box<dyn Future<Output = tinybus::Result<ReadRelayBrainReply>> + Send>>;

/// Own foreground timing and background completion for one admitted turn.
pub(super) async fn run(
    lease: Arc<Lease>,
    request: RelayStartRequest,
    slots: Arc<tokio::sync::Semaphore>,
    connection: tinybus::Connection,
) {
    let Ok(proxy) = connection.proxy(
        request.port.bus_name.as_str(),
        request.port.object_path.as_str(),
        INTERFACE,
    ) else {
        lease.cancel();
        return;
    };
    let invocation = RelayInvocationRequest {
        context_token: request.port.context_token.clone(),
        invocation_id: request.handle.id.clone(),
    };
    let mut finish = FinishGuard {
        runtime: tokio::runtime::Handle::current(),
        proxy: proxy.clone(),
        request: Some(invocation.clone()),
        slots: slots.clone(),
        permit: None,
    };
    let mut delivery = Delivery::default();
    let result = tokio::select! {
        biased;
        ()=lease.wait_cancelled()=>Ok(()),
        result=coordinate(&lease,&request,&proxy,&invocation,slots,&mut finish.permit,&mut delivery)=>result,
    };
    if result.is_err() && !delivery.unacknowledged {
        // A missing completion is a fact, not an invented native reported error.
        // The host applies answerability/delivery policy to this loss.
        let _ = apply(
            &proxy,
            &invocation,
            &mut delivery,
            vec![
                RelayEffect::LostBrain,
                RelayEffect::Error {
                    message: "relay coordination failed".into(),
                },
            ],
        )
        .await;
    }
    let confirmed = finish_host(&proxy, invocation).await;
    finish.request = None;
    if !confirmed && finish.permit.is_some() {
        finish.slots.close();
    }
    finish.permit.take();
    if let Ok(mut phase) = lease.phase.lock() {
        phase.state = RelayState::Closed;
        phase.cleanup_confirmed = confirmed;
    }
}

async fn coordinate(
    lease: &Lease,
    request: &RelayStartRequest,
    proxy: &tinybus::Proxy,
    invocation: &RelayInvocationRequest,
    slots: Arc<tokio::sync::Semaphore>,
    owned_permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
    delivery: &mut Delivery,
) -> tinybus::Result<()> {
    let began = tokio::time::Instant::now();
    let deadline = began + Duration::from_millis(request.acknowledgment_deadline_ms);
    let expiry = began + Duration::from_millis(request.turn_timeout_ms);
    delivery.expiry = Some(expiry);
    let mut foreground_closed = false;
    let mut streamed = false;
    let acquire = slots.acquire_owned();
    tokio::pin!(acquire);
    *owned_permit = Some(loop {
        tokio::select! {
            biased;
            permit=&mut acquire=>break permit.map_err(|_|tinybus::Error::failed("relay scheduler is closed"))?,
            ()=tokio::time::sleep_until(deadline),if !foreground_closed=>{
                acknowledge(lease,request,proxy,invocation,delivery,streamed).await?;foreground_closed=true;
            },
            ()=tokio::time::sleep_until(expiry)=>return lose(request,proxy,invocation,delivery,streamed,foreground_closed).await,
        }
    });

    let admission = tokio::time::timeout(
        CALLBACK_TIMEOUT,
        proxy.call::<StartRelayBrainReply>(START_METHOD, (invocation.clone(),)),
    );
    tokio::pin!(admission);
    let started = loop {
        tokio::select! {
            biased;
            result = &mut admission => break result.map_err(|_| tinybus::Error::failed("relay brain admission exceeded its deadline"))??,
            () = tokio::time::sleep_until(deadline), if !foreground_closed => {
                acknowledge(lease, request, proxy, invocation, delivery, streamed).await?;
                foreground_closed = true;
            },
            () = tokio::time::sleep_until(expiry) => return lose(request, proxy, invocation, delivery, streamed, foreground_closed).await,
        }
    };
    if !started.started {
        return Err(tinybus::Error::failed(
            "relay brain authority is unavailable",
        ));
    }
    if let Ok(mut phase) = lease.phase.lock() {
        phase.state = if foreground_closed {
            RelayState::Deferred
        } else {
            RelayState::Running
        };
    }
    let mut acknowledged = 0_u64;
    let mut incoming = read(proxy.clone(), invocation.clone(), acknowledged);
    loop {
        tokio::select! {
            // Complete already retained progress before closing the foreground;
            // the host appends completion only after its selected deltas.
            biased;
            reply=&mut incoming=>{
                let reply=reply?;
                if !super::super::relay::fits(&reply,1024*1024) {return Err(tinybus::Error::failed("relay progress exceeds its serialized limit"));}
                let Some(event)=reply.event else {
                    if reply.sequence!=acknowledged {return Err(tinybus::Error::failed("idle relay progress advanced its sequence"));}
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    incoming=read(proxy.clone(),invocation.clone(),acknowledged);continue;
                };
                if Some(reply.sequence)!=acknowledged.checked_add(1) {return Err(tinybus::Error::failed("relay progress sequence is not contiguous"));}
                match event {
                    RelayBrainEvent::Delta {text}=>{
                        if text.len()>MAX_TEXT {return Err(tinybus::Error::failed("relay delta exceeds its text limit"));}
                        if !text.is_empty() {apply(proxy,invocation,delivery,vec![RelayEffect::Delta {text}]).await?;streamed=true;}
                    },
                    RelayBrainEvent::Complete {outcome}=>{
                        validate_outcome(&outcome)?;
                        let effects=if foreground_closed {vec![RelayEffect::Deferred {outcome}]} else {complete(outcome,streamed,request)};
                        apply(proxy,invocation,delivery,effects).await?;return Ok(());
                    },
                }
                acknowledged=reply.sequence;incoming=read(proxy.clone(),invocation.clone(),acknowledged);
            },
            ()=tokio::time::sleep_until(deadline),if !foreground_closed=>{
                acknowledge(lease,request,proxy,invocation,delivery,streamed).await?;foreground_closed=true;
            },
            ()=tokio::time::sleep_until(expiry)=>return lose(request,proxy,invocation,delivery,streamed,foreground_closed).await,
        }
    }
}
fn validate_outcome(outcome: &RelayOutcome) -> tinybus::Result<()> {
    let text = match outcome {
        RelayOutcome::Success { text } => text,
        RelayOutcome::Failure { message } => message,
        RelayOutcome::Lost => return Ok(()),
    };
    if text.len() > MAX_TEXT {
        return Err(tinybus::Error::failed(
            "relay completion exceeds its text limit",
        ));
    }
    Ok(())
}
fn complete(
    outcome: RelayOutcome,
    streamed: bool,
    request: &RelayStartRequest,
) -> Vec<RelayEffect> {
    match outcome {
        RelayOutcome::Success { text } => {
            let mut effects = Vec::new();
            if !streamed {
                let text = text.trim();
                effects.push(RelayEffect::Delta {
                    text: if text.is_empty() {
                        request.empty_reply_text.clone()
                    } else {
                        text.into()
                    },
                });
            }
            effects.push(RelayEffect::Done);
            effects
        }
        RelayOutcome::Failure { message } => vec![RelayEffect::Error { message }],
        RelayOutcome::Lost => lost_effects(streamed, request),
    }
}
fn lost_effects(streamed: bool, request: &RelayStartRequest) -> Vec<RelayEffect> {
    let mut effects = vec![RelayEffect::LostBrain];
    if !streamed {
        effects.push(RelayEffect::Delta {
            text: request.lost_reply_text.clone(),
        });
    }
    effects.push(RelayEffect::Done);
    effects
}
async fn lose(
    request: &RelayStartRequest,
    proxy: &tinybus::Proxy,
    invocation: &RelayInvocationRequest,
    delivery: &mut Delivery,
    streamed: bool,
    foreground_closed: bool,
) -> tinybus::Result<()> {
    let effects = if foreground_closed {
        vec![RelayEffect::Deferred {
            outcome: RelayOutcome::Lost,
        }]
    } else {
        lost_effects(streamed, request)
    };
    apply(proxy, invocation, delivery, effects).await
}
async fn acknowledge(
    lease: &Lease,
    request: &RelayStartRequest,
    proxy: &tinybus::Proxy,
    invocation: &RelayInvocationRequest,
    delivery: &mut Delivery,
    streamed: bool,
) -> tinybus::Result<()> {
    let mut effects = Vec::new();
    if !streamed {
        effects.push(RelayEffect::Delta {
            text: request.deadline_reply_text.clone(),
        });
    }
    effects.push(RelayEffect::Done);
    apply(proxy, invocation, delivery, effects).await?;
    if let Ok(mut phase) = lease.phase.lock() {
        phase.state = RelayState::Deferred;
    }
    Ok(())
}
fn read(proxy: tinybus::Proxy, invocation: RelayInvocationRequest, acknowledged: u64) -> Incoming {
    Box::pin(async move {
        tokio::time::timeout(
            CALLBACK_TIMEOUT,
            proxy.call(
                READ_METHOD,
                (ReadRelayBrainRequest {
                    context_token: invocation.context_token,
                    invocation_id: invocation.invocation_id,
                    acknowledged_event: acknowledged,
                },),
            ),
        )
        .await
        .map_err(|_| tinybus::Error::failed("relay progress callback exceeded its deadline"))?
    })
}
async fn apply(
    proxy: &tinybus::Proxy,
    invocation: &RelayInvocationRequest,
    delivery: &mut Delivery,
    effects: Vec<RelayEffect>,
) -> tinybus::Result<()> {
    if effects.is_empty() {
        return Ok(());
    }
    delivery.sequence = delivery
        .sequence
        .checked_add(1)
        .ok_or_else(|| tinybus::Error::failed("relay delivery sequence is exhausted"))?;
    let request = ApplyRelayEffectsRequest {
        context_token: invocation.context_token.clone(),
        invocation_id: invocation.invocation_id.clone(),
        delivery_id: delivery.sequence,
        effects,
    };
    if !super::super::relay::fits(&request, 1024 * 1024) {
        return Err(tinybus::Error::failed(
            "relay effect group exceeds its serialized limit",
        ));
    }
    delivery.unacknowledged = true;
    for attempt in 0..2 {
        if attempt > 0
            && delivery
                .expiry
                .is_some_and(|expiry| tokio::time::Instant::now() >= expiry)
        {
            break;
        }
        if matches!(tokio::time::timeout_at(delivery.expiry.map_or(tokio::time::Instant::now() + CALLBACK_TIMEOUT, |expiry| expiry.min(tokio::time::Instant::now() + CALLBACK_TIMEOUT)),proxy.call::<ApplyRelayEffectsReply>(APPLY_METHOD,(request.clone(),))).await,Ok(Ok(reply)) if reply.applied)
        {
            delivery.unacknowledged = false;
            return Ok(());
        }
    }
    Err(tinybus::Error::failed(
        "relay effects were not acknowledged",
    ))
}
async fn finish_host(proxy: &tinybus::Proxy, request: RelayInvocationRequest) -> bool {
    matches!(tokio::time::timeout(CALLBACK_TIMEOUT,proxy.call::<FinishRelayBrainReply>(FINISH_METHOD,(request,))).await,Ok(Ok(reply)) if reply.finished)
}
struct FinishGuard {
    runtime: tokio::runtime::Handle,
    proxy: tinybus::Proxy,
    request: Option<RelayInvocationRequest>,
    slots: Arc<tokio::sync::Semaphore>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for FinishGuard {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            let proxy = self.proxy.clone();
            let slots = self.slots.clone();
            let permit = self.permit.take();
            self.runtime.spawn(async move {
                if !finish_host(&proxy, request).await && permit.is_some() {
                    slots.close();
                }
                drop(permit);
            });
        }
    }
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
