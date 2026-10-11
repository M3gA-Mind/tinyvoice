//! Native bus fixtures pin scheduling clocks, ordered effects and current authority.

use super::*;
use std::collections::{HashMap, HashSet, VecDeque};
use tinybus::{broker::Broker, transport::memory::MemoryBus};
use tinyvoice_bus::relay::runtime::{RelayHandle, RelayHostPort, RelayLimits};
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Default)]
struct Brain {
    authorities: Arc<Mutex<HashMap<String, String>>>,
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
    admission_cancel: Arc<Notify>,
    admission_exited: Arc<Notify>,
    admission_stopped: Arc<std::sync::atomic::AtomicBool>,
    lose_apply_ack: bool,
    hold_apply: bool,
    finish_entered: Arc<Notify>,
    finish_release: Arc<Notify>,
    wait_finish_release: bool,
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
        if self.authorities.lock().await.get(&request.context_token) != Some(&request.invocation_id)
            || self.reject_authority
        {
            return Ok(StartRelayBrainReply { started: false });
        }
        if self.finishes.lock().await.contains(&request.invocation_id) {
            return Ok(StartRelayBrainReply { started: false });
        }
        self.invocations
            .lock()
            .await
            .entry(request.context_token)
            .or_insert(request.invocation_id);
        self.entered.notify_one();
        if self.hold_start {
            self.admission_cancel.notified().await;
            self.admission_stopped
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.admission_exited.notify_one();
            return Err(tinybus::Error::failed("fixture native admission cancelled"));
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
        if self.authorities.lock().await.get(&request.context_token) != Some(&request.invocation_id)
        {
            return Ok(ApplyRelayEffectsReply { applied: false });
        }
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
        if self.hold_apply {
            std::future::pending::<()>().await;
        }
        if self.lose_apply_ack {
            return Err(tinybus::Error::failed("fixture lost commit acknowledgment"));
        }
        Ok(ApplyRelayEffectsReply { applied: true })
    }
    async fn finish_relay_brain(
        &self,
        request: RelayInvocationRequest,
    ) -> tinybus::Result<FinishRelayBrainReply> {
        if self.authorities.lock().await.get(&request.context_token) != Some(&request.invocation_id)
        {
            return Ok(FinishRelayBrainReply { finished: false });
        }
        self.finish_entered.notify_one();
        if self.wait_finish_release {
            self.finish_release.notified().await;
        }
        if self.hold_finish {
            std::future::pending::<()>().await;
        }
        if self.hold_start
            && !self
                .admission_stopped
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.admission_cancel.notify_one();
            self.admission_exited.notified().await;
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
    start(&turns, &brain, request(handle.clone(), "pending", 100)).await?;
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
    turns.release(handle).await?;
    assert!(
        brain
            .admission_stopped
            .load(std::sync::atomic::Ordering::SeqCst)
    );
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
    start(&turns, &brain, request.clone()).await?;
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
    start(&turns, &brain, request(first.clone(), "first", 5000)).await?;
    brain.entered.notified().await;
    let second = turns.reserve()?.handle;
    start(&turns, &brain, request(second.clone(), "second", 100)).await?;
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

/// The host binds the returned module lease to its existing saved context.
/// Neither a callback nor a provider-selected id can manufacture authority.
async fn start(
    turns: &super::super::table::Turns,
    brain: &Brain,
    request: RelayStartRequest,
) -> tinybus::Result<()> {
    brain.authorities.lock().await.insert(
        request.port.context_token.clone(),
        request.handle.id.clone(),
    );
    turns.start(request)?;
    Ok(())
}

#[tokio::test]
async fn native_completion_fallback_failure_and_lost_work_preserve_foreground_effects()
-> tinybus::Result<()> {
    for (outcome, expected) in [
        (
            RelayOutcome::Success { text: "   ".into() },
            vec![
                RelayEffect::Delta {
                    text: "pause".into(),
                },
                RelayEffect::Done,
            ],
        ),
        (
            RelayOutcome::Success {
                text: " answer ".into(),
            },
            vec![
                RelayEffect::Delta {
                    text: "answer".into(),
                },
                RelayEffect::Done,
            ],
        ),
        (
            RelayOutcome::Failure {
                message: "reported failure".into(),
            },
            vec![RelayEffect::Error {
                message: "reported failure".into(),
            }],
        ),
        (
            RelayOutcome::Lost,
            vec![
                RelayEffect::LostBrain,
                RelayEffect::Delta {
                    text: "lost".into(),
                },
                RelayEffect::Done,
            ],
        ),
    ] {
        let brain = Brain::default();
        brain.events.lock().await.insert(
            "case".into(),
            VecDeque::from([
                RelayBrainEvent::Delta {
                    text: String::new(),
                },
                RelayBrainEvent::Complete { outcome },
            ]),
        );
        let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
        let handle = turns.reserve()?.handle;
        start(&turns, &brain, request(handle.clone(), "case", 1000)).await?;
        tokio::time::timeout(Duration::from_secs(1), brain.finished.notified())
            .await
            .map_err(|_| tinybus::Error::failed("fixture finish deadline"))?;
        assert_eq!(brain.effects.lock().await["case"], expected);
        turns.release(handle).await?;
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn release_expires_unresponsive_host_cleanup_without_claiming_success() -> tinybus::Result<()>
{
    let brain = Brain {
        hold_finish: true,
        ..Brain::default()
    };
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    start(&turns, &brain, request(handle.clone(), "known", 1000)).await?;
    brain.entered.notified().await;
    let began = tokio::time::Instant::now();
    assert!(turns.release(handle.clone()).await.is_err());
    assert!(began.elapsed() <= Duration::from_secs(2));
    assert!(brain.finishes.lock().await.is_empty());
    assert_eq!(turns.release(handle).await?.state, RelayState::Closed);
    Ok(())
}

#[tokio::test]
async fn unknown_or_currently_revoked_native_authority_never_starts_an_executor()
-> tinybus::Result<()> {
    let brain = Brain {
        reject_authority: true,
        ..Brain::default()
    };
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    start(&turns, &brain, request(handle.clone(), "revoked", 1000)).await?;
    tokio::time::timeout(Duration::from_secs(1), brain.finished.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture finish deadline"))?;
    assert!(brain.invocations.lock().await.is_empty());
    assert!(
        brain.effects.lock().await["revoked"]
            .iter()
            .any(|effect| matches!(effect, RelayEffect::Error { .. }))
    );
    assert!(
        !brain.effects.lock().await["revoked"]
            .iter()
            .any(|effect| matches!(
                effect,
                RelayEffect::Deferred {
                    outcome: RelayOutcome::Success { .. }
                }
            ))
    );
    turns.release(handle).await?;
    Ok(())
}

#[tokio::test]
async fn dropping_the_scheduler_notifies_only_the_existing_host_invocation() -> tinybus::Result<()>
{
    let brain = Brain::default();
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    start(&turns, &brain, request(handle.clone(), "drop", 1000)).await?;
    brain.entered.notified().await;
    drop(turns);
    tokio::time::timeout(Duration::from_secs(1), brain.finished.notified())
        .await
        .map_err(|_| tinybus::Error::failed("fixture drop cleanup deadline"))?;
    assert_eq!(brain.finishes.lock().await.len(), 1);
    assert!(brain.finishes.lock().await.contains(&handle.id));
    Ok(())
}

#[tokio::test]
async fn unknown_and_late_finish_cannot_create_or_revive_native_work() -> tinybus::Result<()> {
    let brain = Brain::default();
    let (turns, module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    let request = request(handle.clone(), "known", 1000);
    brain
        .authorities
        .lock()
        .await
        .insert("known".into(), handle.id.clone());
    let proxy = module.proxy("ai.test.Relay", "/test/Relay", INTERFACE)?;
    let invocation = RelayInvocationRequest {
        context_token: "known".into(),
        invocation_id: handle.id.clone(),
    };
    assert!(finish_host(&proxy, invocation.clone()).await);
    assert!(finish_host(&proxy, invocation).await);
    assert!(
        !finish_host(
            &proxy,
            RelayInvocationRequest {
                context_token: "unknown".into(),
                invocation_id: handle.id.clone()
            }
        )
        .await
    );
    assert!(
        !finish_host(
            &proxy,
            RelayInvocationRequest {
                context_token: "known".into(),
                invocation_id: "invented".into()
            }
        )
        .await
    );
    assert!(brain.invocations.lock().await.is_empty());
    assert_eq!(brain.finishes.lock().await.len(), 1);
    // This known context is already finished; the fixture must not reopen it.
    let reply: StartRelayBrainReply = proxy
        .call(
            START_METHOD,
            (RelayInvocationRequest {
                context_token: request.port.context_token,
                invocation_id: handle.id.clone(),
            },),
        )
        .await?;
    assert!(!reply.started);
    turns.release(handle).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn overall_deadline_finishes_queued_and_active_turns_without_inventing_native_completion()
-> tinybus::Result<()> {
    for queued in [false, true] {
        let brain = Brain::default();
        let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
        let first = if queued {
            let first = turns.reserve()?.handle;
            start(&turns, &brain, request(first.clone(), "occupant", 5000)).await?;
            brain.entered.notified().await;
            Some(first)
        } else {
            None
        };
        let handle = turns.reserve()?.handle;
        let mut request = request(handle.clone(), "expiring", 10);
        request.turn_timeout_ms = 100;
        start(&turns, &brain, request).await?;
        if !queued {
            brain.entered.notified().await;
        }
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if brain.finishes.lock().await.contains(&handle.id) {
                    break;
                }
                brain.finished.notified().await;
            }
        })
        .await
        .map_err(|_| tinybus::Error::failed("fixture overall expiry deadline"))?;
        assert_eq!(
            brain.effects.lock().await["expiring"],
            [
                RelayEffect::Delta {
                    text: "working".into()
                },
                RelayEffect::Done,
                RelayEffect::Deferred {
                    outcome: RelayOutcome::Lost
                },
            ]
        );
        assert_eq!(
            brain.invocations.lock().await.contains_key("expiring"),
            !queued
        );
        turns.release(handle).await?;
        if let Some(first) = first {
            turns.release(first).await?;
        }
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exhausted_commit_acknowledgments_do_not_emit_contradictory_new_effects()
-> tinybus::Result<()> {
    let brain = Brain {
        lose_apply_ack: true,
        ..Brain::default()
    };
    brain.events.lock().await.insert(
        "committed".into(),
        VecDeque::from([RelayBrainEvent::Complete {
            outcome: RelayOutcome::Success {
                text: "answer".into(),
            },
        }]),
    );
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    start(&turns, &brain, request(handle.clone(), "committed", 1000)).await?;
    brain.finished.notified().await;
    assert_eq!(
        brain.effects.lock().await["committed"],
        [
            RelayEffect::Delta {
                text: "answer".into()
            },
            RelayEffect::Done
        ]
    );
    assert_eq!(brain.committed.lock().await.len(), 1);
    turns.release(handle).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn native_execution_slot_is_retained_until_finish_confirms_stopped_work()
-> tinybus::Result<()> {
    let brain = Brain {
        wait_finish_release: true,
        ..Brain::default()
    };
    brain.events.lock().await.insert(
        "first".into(),
        VecDeque::from([RelayBrainEvent::Complete {
            outcome: RelayOutcome::Success {
                text: "answer".into(),
            },
        }]),
    );
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let first = turns.reserve()?.handle;
    start(&turns, &brain, request(first.clone(), "first", 1000)).await?;
    brain.finish_entered.notified().await;
    let second = turns.reserve()?.handle;
    start(&turns, &brain, request(second.clone(), "second", 1000)).await?;
    tokio::time::advance(Duration::from_millis(100)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(!brain.invocations.lock().await.contains_key("second"));
    brain.finish_release.notify_one();
    brain.finished.notified().await;
    for _ in 0..100 {
        if brain.invocations.lock().await.contains_key("second") {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(brain.invocations.lock().await.contains_key("second"));
    turns.release(first).await?;
    brain.finish_release.notify_one();
    turns.release(second).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn overall_deadline_cancels_stalled_effect_delivery_before_callback_retry_budget()
-> tinybus::Result<()> {
    let brain = Brain {
        hold_apply: true,
        ..Brain::default()
    };
    brain.events.lock().await.insert(
        "deadline".into(),
        VecDeque::from([RelayBrainEvent::Delta {
            text: "selected".into(),
        }]),
    );
    let (turns, _module, _host) = fixture(brain.clone(), 1).await?;
    let handle = turns.reserve()?.handle;
    let mut req = request(handle.clone(), "deadline", 100);
    req.turn_timeout_ms = 200;
    start(&turns, &brain, req).await?;
    brain.applied.notified().await;
    tokio::time::timeout(Duration::from_millis(350), brain.finished.notified())
        .await
        .map_err(|_| {
            tinybus::Error::failed("overall deadline was suspended during effect delivery")
        })?;
    assert_eq!(
        brain.effects.lock().await["deadline"],
        [RelayEffect::Delta {
            text: "selected".into()
        }]
    );
    turns.release(handle).await?;
    Ok(())
}
