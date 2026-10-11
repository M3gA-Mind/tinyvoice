//! Loaded-artifact relay proof with admitted native fixture work and mechanical callbacks.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    sync::Arc,
    time::Duration,
};
use tinyvoice_bus::relay::runtime::{
    ApplyRelayEffectsReply, ApplyRelayEffectsRequest, FinishRelayBrainReply, ReadRelayBrainReply,
    ReadRelayBrainRequest, RelayBrainEvent, RelayEffect, RelayHandleRequest, RelayHostPort,
    RelayInvocationRequest, RelayLimits, RelayOutcome, RelayStartRequest, RelayState, RelayStatus,
    StartRelayBrainReply,
};
use tokio::sync::{Mutex, Notify};
type CheckResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Default)]
struct NativeBrain {
    expected: Arc<Mutex<HashMap<String, String>>>,
    tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    events: Arc<Mutex<HashMap<String, VecDeque<RelayBrainEvent>>>>,
    retained: Arc<Mutex<HashMap<String, (u64, RelayBrainEvent)>>>,
    effects: Arc<Mutex<HashMap<String, Vec<RelayEffect>>>>,
    committed: Arc<Mutex<HashSet<(String, u64)>>>,
    starts: Arc<Mutex<usize>>,
    completed: Arc<Notify>,
    continue_late: Arc<Notify>,
    finished: Arc<Mutex<HashSet<String>>>,
}
#[tinybus::interface(name = "ai.tinyhumans.tinyvoice.RelayHost")]
impl NativeBrain {
    async fn start_relay_brain(
        &self,
        request: RelayInvocationRequest,
    ) -> tinybus::Result<StartRelayBrainReply> {
        if self.expected.lock().await.get(&request.context_token) != Some(&request.invocation_id)
            || request.context_token == "revoked"
            || self.finished.lock().await.contains(&request.invocation_id)
        {
            return Ok(StartRelayBrainReply { started: false });
        }
        let mut tasks = self.tasks.lock().await;
        if tasks.contains_key(&request.invocation_id) {
            return Ok(StartRelayBrainReply { started: true });
        }
        *self.starts.lock().await += 1;
        let context = request.context_token;
        let events = self.events.clone();
        let continue_late = self.continue_late.clone();
        tasks.insert(
            request.invocation_id,
            tokio::spawn(async move {
                if context == "delayed" {
                    continue_late.notified().await;
                }
                // Native host work computes its reply; the module only coordinates
                // selected progress and completion. No tool execution is invented.
                let words = ["module", " ", "relay"];
                let reply = words.concat();
                let mut events = events.lock().await;
                let queue = events.entry(context).or_default();
                queue.extend(words.map(|text| RelayBrainEvent::Delta { text: text.into() }));
                queue.push_back(RelayBrainEvent::Complete {
                    outcome: RelayOutcome::Success { text: reply },
                });
            }),
        );
        Ok(StartRelayBrainReply { started: true })
    }
    async fn read_relay_brain(
        &self,
        request: ReadRelayBrainRequest,
    ) -> tinybus::Result<ReadRelayBrainReply> {
        if self.expected.lock().await.get(&request.context_token) != Some(&request.invocation_id) {
            return Err(tinybus::Error::failed("unknown native fixture context"));
        }
        let mut retained = self.retained.lock().await;
        if retained
            .get(&request.context_token)
            .is_some_and(|(sequence, _)| *sequence == request.acknowledged_event)
        {
            retained.remove(&request.context_token);
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
        if self.expected.lock().await.get(&request.context_token) != Some(&request.invocation_id) {
            return Ok(ApplyRelayEffectsReply { applied: false });
        }
        let fresh = self
            .committed
            .lock()
            .await
            .insert((request.invocation_id, request.delivery_id));
        if fresh {
            if request.context_token == "delayed" && request.effects.contains(&RelayEffect::Done) {
                self.continue_late.notify_one();
            }
            self.effects
                .lock()
                .await
                .entry(request.context_token)
                .or_default()
                .extend(request.effects);
            // Lose the first reply after effects commit. A retry must reuse its
            // identity and must not duplicate an already committed delta.
            if request.delivery_id == 1 {
                return Err(tinybus::Error::failed("fixture lost effect acknowledgment"));
            }
        }
        Ok(ApplyRelayEffectsReply { applied: true })
    }
    async fn finish_relay_brain(
        &self,
        request: RelayInvocationRequest,
    ) -> tinybus::Result<FinishRelayBrainReply> {
        if self.expected.lock().await.get(&request.context_token) != Some(&request.invocation_id) {
            return Ok(FinishRelayBrainReply { finished: false });
        }
        if let Some(mut task) = self.tasks.lock().await.remove(&request.invocation_id) {
            // Native work is already complete on the happy path; cleanup also
            // owns cancellation if the module ends this invocation early.
            if !task.is_finished() {
                task.abort();
            }
            let _ = (&mut task).await;
        }
        self.finished.lock().await.insert(request.invocation_id);
        self.completed.notify_one();
        Ok(FinishRelayBrainReply { finished: true })
    }
}
pub(super) async fn verify(
    proxy: &tinybus::Proxy,
    client: &tinybus::Connection,
) -> CheckResult<()> {
    let brain = NativeBrain::default();
    client
        .serve_at("/test/RelayHost".try_into()?, brain.clone())
        .await?;
    client.request_name("ai.test.RelayHost").await?;
    let limits = RelayLimits {
        max_active_turns: 1,
        max_pending_turns: 8,
    };
    let configured: RelayLimits = proxy.call("RelayConfigure", (limits,)).await?;
    if configured != limits {
        return Err(io::Error::other("relay scheduling limits changed").into());
    }
    for context in ["foreground", "delayed", "revoked"] {
        let reserved: RelayStatus = proxy.call("RelayReserve", ()).await?;
        brain
            .expected
            .lock()
            .await
            .insert(context.into(), reserved.handle.id.clone());
        let request = RelayStartRequest {
            handle: reserved.handle.clone(),
            port: RelayHostPort {
                bus_name: "ai.test.RelayHost".into(),
                object_path: "/test/RelayHost".into(),
                context_token: context.into(),
            },
            acknowledgment_deadline_ms: if context == "delayed" { 20 } else { 1000 },
            turn_timeout_ms: 3000,
            empty_reply_text: "pause".into(),
            deadline_reply_text: "working".into(),
            lost_reply_text: "lost".into(),
        };
        let _: RelayStatus = proxy.call("RelayStart", (request.clone(),)).await?;
        let _: RelayStatus = proxy.call("RelayStart", (request,)).await?;
        tokio::time::timeout(Duration::from_secs(5), brain.completed.notified()).await?;
        let effects = brain
            .effects
            .lock()
            .await
            .get(context)
            .cloned()
            .unwrap_or_default();
        let expected = match context {
            "foreground" => vec![
                RelayEffect::Delta {
                    text: "module".into(),
                },
                RelayEffect::Delta { text: " ".into() },
                RelayEffect::Delta {
                    text: "relay".into(),
                },
                RelayEffect::Done,
            ],
            "delayed" => vec![
                RelayEffect::Delta {
                    text: "working".into(),
                },
                RelayEffect::Done,
                RelayEffect::Delta {
                    text: "module".into(),
                },
                RelayEffect::Delta { text: " ".into() },
                RelayEffect::Delta {
                    text: "relay".into(),
                },
                RelayEffect::Deferred {
                    outcome: RelayOutcome::Success {
                        text: "module relay".into(),
                    },
                },
            ],
            _ => vec![
                RelayEffect::LostBrain,
                RelayEffect::Error {
                    message: "relay coordination failed".into(),
                },
            ],
        };
        if effects != expected {
            return Err(
                io::Error::other(format!("relay effect order changed for {context}")).into(),
            );
        }
        let released: RelayStatus = proxy
            .call(
                "RelayRelease",
                (RelayHandleRequest {
                    handle: reserved.handle.clone(),
                },),
            )
            .await?;
        if released.state != RelayState::Closed {
            return Err(io::Error::other("relay release did not close").into());
        }
        let _: RelayStatus = proxy
            .call(
                "RelayRelease",
                (RelayHandleRequest {
                    handle: reserved.handle,
                },),
            )
            .await?;
    }
    if *brain.starts.lock().await != 2 {
        return Err(
            io::Error::other("retry or revoked authority started another native brain").into(),
        );
    }
    let _: () = proxy.call("RelayShutdown", ()).await?;
    if proxy.call::<RelayStatus>("RelayReserve", ()).await.is_ok() {
        return Err(io::Error::other("relay shutdown reopened admission").into());
    }
    Ok(())
}
