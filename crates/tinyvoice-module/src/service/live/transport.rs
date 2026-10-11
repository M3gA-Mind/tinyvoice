//! Module-owned client/provider select loop over mechanical host callbacks.

use super::table::{
    Lease, MAX_AUDIO_BYTES, MAX_BATCH_BYTES, MAX_TEXT_BYTES, read_lease, reservation_expired,
    send_lease, stop_provider,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tinyvoice_bus::live::{
    ClientFrame, LiveTransportAction, PrepareLiveInputRequest, PreparedLiveInput, TranscriptRole,
    session::{LiveCommand, LiveHandle, LiveOutput, LiveReadRequest, LiveSendRequest, LiveState},
    transport::{
        CLOSE_METHOD, CloseLiveTransportReply, CloseLiveTransportRequest, DELIVER_METHOD,
        DeliverLiveEffectsReply, DeliverLiveEffectsRequest, INTERFACE, LiveTransportPort,
        READ_METHOD, ReadLiveInputReply, ReadLiveInputRequest,
    },
};

const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
type Incoming = Pin<Box<dyn Future<Output = tinybus::Result<ReadLiveInputReply>> + Send>>;

/// Own the client coordinator until EOF, write failure, cancellation or native close.
pub(super) async fn run(
    lease: Arc<Lease>,
    handle: LiveHandle,
    port: LiveTransportPort,
    connection: tinybus::Connection,
) {
    let Ok(proxy) = connection.proxy(port.bus_name.as_str(), port.object_path.as_str(), INTERFACE)
    else {
        lease.cancel();
        return;
    };
    // Select around the whole loop so cancellation interrupts callback waits,
    // persistence acknowledgments and bounded native input admission as well.
    tokio::select! {
        biased;
        () = lease.wait_cancelled() => {}
        _ = coordinate(&lease, &handle, &port, &proxy) => {}
    }
    let _ = stop_provider(&lease).await;
    let closed = tokio::time::timeout(
        CALLBACK_TIMEOUT,
        proxy.call::<CloseLiveTransportReply>(
            CLOSE_METHOD,
            (CloseLiveTransportRequest {
                context_token: port.context_token,
            },),
        ),
    )
    .await;
    if let Ok(mut phase) = lease.phase.lock() {
        phase.transport_cleanup_confirmed = matches!(closed, Ok(Ok(reply)) if reply.closed);
    }
}

async fn coordinate(
    lease: &Lease,
    handle: &LiveHandle,
    port: &LiveTransportPort,
    proxy: &tinybus::Proxy,
) -> tinybus::Result<()> {
    let mut input_ack = 0_u64;
    let mut output_ack = None;
    let mut input: Incoming = Box::pin(std::future::pending());
    let mut input_pending = false;
    let mut clock = tokio::time::interval(Duration::from_millis(10));
    clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            // Drain ready provider outputs even if an idle host input callback
            // replies immediately, preventing starvation by empty polls.
            biased;
            _ = clock.tick() => {
                let batch = read_lease(lease, LiveReadRequest { handle: handle.clone(), acknowledged_batch: output_ack }).await?;
                if !batch.outputs.is_empty() {
                    deliver(proxy, port, handle, format!("output-{}", batch.batch), batch.outputs).await?;
                    output_ack = Some(batch.batch);
                } else if batch.state == LiveState::Closed {
                    return Ok(());
                }
                if batch.state == LiveState::Reserved && reservation_expired(lease) {
                    return Ok(());
                }
                if !input_pending && batch.state == LiveState::Running {
                    input = next_input(proxy.clone(), port.clone(), input_ack);
                    input_pending = true;
                }
            }
            reply = &mut input => {
                input_pending = false;
                input = Box::pin(std::future::pending());
                let reply = reply?;
                if !super::super::relay::fits(&reply, MAX_BATCH_BYTES) {
                    return Err(tinybus::Error::failed("live client frame exceeds its serialized limit"));
                }
                let Some(frame) = reply.frame else {
                    if reply.sequence != input_ack { return Err(tinybus::Error::failed("idle live frame advanced its sequence")); }
                    continue;
                };
                let expected = input_ack.checked_add(1).ok_or_else(|| tinybus::Error::failed("live client frame sequence is exhausted"))?;
                if reply.sequence != expected { return Err(tinybus::Error::failed("live client frame sequence is not contiguous")); }
                let prepared = tinyvoice::live::prepare_input(PrepareLiveInputRequest { awaiting_start: false, frame })
                    .map_err(|error| tinybus::Error::failed(error.to_string()))?;
                if !apply_input(lease, handle, port, proxy, reply.sequence, prepared).await? {
                    return Ok(());
                }
                input_ack = reply.sequence;
            }
        }
    }
}

fn next_input(proxy: tinybus::Proxy, port: LiveTransportPort, ack: u64) -> Incoming {
    Box::pin(async move {
        tokio::time::timeout(
            CALLBACK_TIMEOUT,
            proxy.call(
                READ_METHOD,
                (ReadLiveInputRequest {
                    context_token: port.context_token,
                    acknowledged_frame: ack,
                },),
            ),
        )
        .await
        .map_err(|_| tinybus::Error::failed("live client input callback exceeded its deadline"))?
    })
}

/// Preserve typed-text persistence before dispatch and native stop semantics.
async fn apply_input(
    lease: &Lease,
    handle: &LiveHandle,
    port: &LiveTransportPort,
    proxy: &tinybus::Proxy,
    sequence: u64,
    input: PreparedLiveInput,
) -> tinybus::Result<bool> {
    let command = match input {
        PreparedLiveInput::Control(ClientFrame::Text { text }) => {
            if text.len() > MAX_TEXT_BYTES {
                return Err(tinybus::Error::failed("live text exceeds its input limit"));
            }
            deliver(
                proxy,
                port,
                handle,
                format!("input-{sequence}"),
                vec![LiveOutput::Persist {
                    role: TranscriptRole::User,
                    text: text.clone(),
                }],
            )
            .await?;
            LiveCommand::Text { text }
        }
        PreparedLiveInput::Control(ClientFrame::Interrupt) => LiveCommand::Interrupt,
        PreparedLiveInput::Control(ClientFrame::Stop) => LiveCommand::Stop,
        PreparedLiveInput::Transport(LiveTransportAction::Audio { pcm16 }) => {
            if pcm16.len() > MAX_AUDIO_BYTES {
                return Err(tinybus::Error::failed("live audio exceeds its input limit"));
            }
            LiveCommand::Audio {
                pcm16_base64: BASE64.encode(pcm16),
            }
        }
        PreparedLiveInput::Transport(LiveTransportAction::Closed) => return Ok(false),
        PreparedLiveInput::Control(ClientFrame::Start { .. })
        | PreparedLiveInput::Transport(LiveTransportAction::Ignore) => return Ok(true),
    };
    send_lease(
        lease,
        LiveSendRequest {
            handle: handle.clone(),
            command,
        },
    )
    .await?;
    Ok(true)
}

/// Retry a lost acknowledgement with the same host idempotency identity.
async fn deliver(
    proxy: &tinybus::Proxy,
    port: &LiveTransportPort,
    handle: &LiveHandle,
    delivery_id: String,
    outputs: Vec<LiveOutput>,
) -> tinybus::Result<()> {
    let request = DeliverLiveEffectsRequest {
        context_token: port.context_token.clone(),
        handle: handle.clone(),
        delivery_id,
        outputs,
    };
    if !super::super::relay::fits(&request, 2 * MAX_BATCH_BYTES) {
        return Err(tinybus::Error::failed(
            "live transport delivery exceeds its serialized limit",
        ));
    }
    for _ in 0..2 {
        if matches!(tokio::time::timeout(CALLBACK_TIMEOUT, proxy.call::<DeliverLiveEffectsReply>(
            DELIVER_METHOD, (request.clone(),),
        )).await, Ok(Ok(reply)) if reply.delivered)
        {
            return Ok(());
        }
    }
    Err(tinybus::Error::failed(
        "live transport did not acknowledge delivery",
    ))
}
