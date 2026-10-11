//! Native bus fixtures pin scheduling clocks, ordered effects and current authority.

use super::*;
use std::collections::{HashMap, HashSet, VecDeque};
use tinybus::{broker::Broker, transport::memory::MemoryBus};
use tinyvoice_bus::relay::runtime::{RelayHandle, RelayHostPort, RelayLimits};
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Default)]
struct Brain {
    invocations: Arc<Mutex<HashMap<String, String>>>,
    events: Arc<Mutex<HashMap<String, VecDeque<RelayBrainEvent>>>>,
    retained: Arc<Mutex<HashMap<String, (u64, RelayBrainEvent)>>>,
    acknowledged: Arc<Mutex<HashMap<String, u64>>>,
    effects: Arc<Mutex<HashMap<String, Vec<RelayEffect>>>>,
    committed: Arc<Mutex<HashSet<(String, u64)>>>,
    finishes: Arc<Mutex<HashSet<String>>>,
    entered: Arc<Notify>,
    applied: Arc<Notify>,
    finished: Arc<Notify>,
    hold_start: bool,
    hold_finish: bool,
    reject_authority: bool,
}
#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.RelayHost")]
impl Brain {
    async fn start_relay_brain(
        &self,
        request: RelayInvocationRequest,
    ) -> tinybus::Result<StartRelayBrainReply> {
        if self.reject_authority {
            return Ok(StartRelayBrainReply { started: false });
        }
        self.invocations
            .lock()
            .await
            .entry(request.context_token)
            .or_insert(request.invocation_id);
        self.entered.notify_one();
        if self.hold_start {
            std::future::pending::<()>().await;
        }
        Ok(StartRelayBrainReply { started: true })
    }
    async fn read_relay_brain(
        &self,
        request: ReadRelayBrainRequest,
    ) -> tinybus::Result<ReadRelayBrainReply> {
        if self.invocations.lock().await.get(&request.context_token) != Some(&request.invocation_id)
        {
            return Err(tinybus::Error::failed("fixture unknown invocation"));
        }
        let mut retained = self.retained.lock().await;
        if retained
            .get(&request.context_token)
            .is_some_and(|(sequence, _)| *sequence == request.acknowledged_event)
        {
            retained.remove(&request.context_token);
            self.acknowledged
                .lock()
                .await
                .insert(request.context_token.clone(), request.acknowledged_event);
        }
        if !retained.contains_key(&request.context_token)
            && let Some(event) = self
                .events
                .lock()
                .await
                .entry(request.context_token.clone())
                .or_default()
                .pop_front()
        {
            retained.insert(
                request.context_token.clone(),
                (request.acknowledged_event + 1, event),
            );
        }
        Ok(match retained.get(&request.context_token) {
            Some((sequence, event)) => ReadRelayBrainReply {
                sequence: *sequence,
                event: Some(event.clone()),
            },
            None => ReadRelayBrainReply {
                sequence: request.acknowledged_event,
                event: None,
            },
        })
    }
    async fn apply_relay_effects(
        &self,
        request: ApplyRelayEffectsRequest,
    ) -> tinybus::Result<ApplyRelayEffectsReply> {
        if self
            .committed
            .lock()
            .await
            .insert((request.invocation_id, request.delivery_id))
        {
            self.effects
                .lock()
                .await
                .entry(request.context_token)
                .or_default()
                .extend(request.effects);
        }
        self.applied.notify_one();
        Ok(ApplyRelayEffectsReply { applied: true })
    }
    async fn finish_relay_brain(
        &self,
        request: RelayInvocationRequest,
    ) -> tinybus::Result<FinishRelayBrainReply> {
        if self.invocations.lock().await.get(&request.context_token) != Some(&request.invocation_id)
        {
            return Ok(FinishRelayBrainReply { finished: false });
        }
        if self.hold_finish {
            std::future::pending::<()>().await;
        }
        self.finishes.lock().await.insert(request.invocation_id);
        self.finished.notify_one();
        Ok(FinishRelayBrainReply { finished: true })
    }
}
async fn fixture(
    brain: Brain,
    active: u16,
) -> tinybus::Result<(
    super::super::table::Turns,
    tinybus::Connection,
    tinybus::Connection,
)> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let host = tinybus::Connection::connect(bus.connect().await?).await?;
    host.serve_at("/test/Relay".try_into()?, brain).await?;
    host.request_name("ai.test.Relay").await?;
    let module = tinybus::Connection::connect(bus.connect().await?).await?;
    let turns = super::super::table::Turns::with_connection(&module);
    turns.configure(RelayLimits {
        max_active_turns: active,
        max_pending_turns: 16,
    })?;
    Ok((turns, module, host))
}
fn request(handle: RelayHandle, context: &str, deadline: u64) -> RelayStartRequest {
    RelayStartRequest {
        handle,
        port: RelayHostPort {
            bus_name: "ai.test.Relay".into(),
            object_path: "/test/Relay".into(),
            context_token: context.into(),
        },
        acknowledgment_deadline_ms: deadline,
        turn_timeout_ms: 10_000,
        empty_reply_text: "pause".into(),
        deadline_reply_text: "working".into(),
        lost_reply_text: "lost".into(),
    }
}
async fn wait_effect(brain: &Brain, context: &str) -> tinybus::Result<Vec<RelayEffect>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let effects = brain
                .effects
                .lock()
                .await
                .get(context)
                .cloned()
                .unwrap_or_default();
            if !effects.is_empty() {
                return effects;
            }
            brain.applied.notified().await;
        }
    })
    .await
    .map_err(|_| tinybus::Error::failed("fixture effect deadline"))
}

#[tokio::test(start_paused = true)]
async fn acknowledgment_deadline_remains_live_while_the_host_start_callback_is_pending()
-> tinybus::Result<()> {
    let brain = Brain {
        hold_start: true,
        ..Brain::default()
    };
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    turns.start(request(handle.clone(), "pending", 100))?;
    brain.entered.notified().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    let effects = tokio::time::timeout(Duration::from_millis(350), wait_effect(&brain, "pending"))
        .await
        .map_err(|_| tinybus::Error::failed("acknowledgment waited for native admission"))??;
    assert_eq!(
        effects,
        [
            RelayEffect::Delta {
                text: "working".into()
            },
            RelayEffect::Done
        ]
    );
    let _ = turns.release(handle).await;
    Ok(())
}

#[tokio::test]
async fn selected_deltas_preserve_whitespace_and_precede_done_without_whole_reply_duplication()
-> tinybus::Result<()> {
    let brain = Brain::default();
    brain.events.lock().await.insert(
        "stream".into(),
        VecDeque::from([
            RelayBrainEvent::Delta { text: "one".into() },
            RelayBrainEvent::Delta { text: " ".into() },
            RelayBrainEvent::Delta { text: "two".into() },
            RelayBrainEvent::Complete {
                outcome: RelayOutcome::Success {
                    text: "whole reply".into(),
                },
            },
        ]),
    );
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    let request = request(handle.clone(), "stream", 1000);
    turns.start(request.clone())?;
    turns.start(request)?;
    tokio::time::timeout(Duration::from_secs(1), brain.finished.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture finish deadline"))?;
    assert_eq!(
        brain.effects.lock().await["stream"],
        [
            RelayEffect::Delta { text: "one".into() },
            RelayEffect::Delta { text: " ".into() },
            RelayEffect::Delta { text: "two".into() },
            RelayEffect::Done,
        ]
    );
    assert_eq!(brain.invocations.lock().await.len(), 1);
    turns.release(handle.clone()).await?;
    turns.release(handle).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn queued_turn_acknowledges_before_a_native_execution_slot_becomes_available()
-> tinybus::Result<()> {
    let brain = Brain::default();
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let first = turns.reserve()?.handle;
    turns.start(request(first.clone(), "first", 5000))?;
    brain.entered.notified().await;
    let second = turns.reserve()?.handle;
    turns.start(request(second.clone(), "second", 100))?;
    tokio::time::advance(Duration::from_millis(100)).await;
    assert_eq!(
        wait_effect(&brain, "second").await?,
        [
            RelayEffect::Delta {
                text: "working".into()
            },
            RelayEffect::Done
        ]
    );
    assert!(!brain.invocations.lock().await.contains_key("second"));
    turns.release(first).await?;
    brain.entered.notified().await;
    brain.events.lock().await.insert(
        "second".into(),
        VecDeque::from([
            RelayBrainEvent::Delta {
                text: "late".into(),
            },
            RelayBrainEvent::Complete {
                outcome: RelayOutcome::Success {
                    text: "answer".into(),
                },
            },
        ]),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if brain
                .effects
                .lock()
                .await
                .get("second")
                .is_some_and(|effects| {
                    effects
                        .iter()
                        .any(|effect| matches!(effect, RelayEffect::Deferred { .. }))
                })
            {
                break;
            }
            brain.applied.notified().await;
        }
    })
    .await
    .map_err(|_| tinybus::Error::failed("fixture deferred deadline"))?;
    assert_eq!(
        brain.effects.lock().await["second"],
        [
            RelayEffect::Delta {
                text: "working".into()
            },
            RelayEffect::Done,
            RelayEffect::Delta {
                text: "late".into()
            },
            RelayEffect::Deferred {
                outcome: RelayOutcome::Success {
                    text: "answer".into()
                }
            },
        ]
    );
    turns.release(second).await?;
    Ok(())
}
