//! Mechanical client socket and persistence callback contract for the module pump.

use super::{
    TransportFrame,
    session::{LiveHandle, LiveOutput},
};
use serde::{Deserialize, Serialize};

/// Host interface providing mechanical socket/persistence callbacks.
pub const INTERFACE: &str = "ai.tinyhumans.tinyvoice.LiveTransport";
/// Read one replayable client frame without applying provider or session policy.
pub const READ_METHOD: &str = "ReadLiveInput";
/// Apply one ordered effect group idempotently.
pub const DELIVER_METHOD: &str = "DeliverLiveEffects";
/// Idempotently close a known client socket/context.
pub const CLOSE_METHOD: &str = "CloseLiveTransport";

/// Opaque host transport endpoint; the module owns the generic coordinator.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTransportPort {
    /// Host bus name.
    pub bus_name: String,
    /// Host object path.
    pub object_path: String,
    /// Saved known host transport context, never provider-selected.
    pub context_token: String,
}

impl std::fmt::Debug for LiveTransportPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveTransportPort")
            .field("bus_name", &self.bus_name)
            .field("object_path", &self.object_path)
            .field("context_token", &"[redacted]")
            .finish()
    }
}

/// Attach one transport coordinator to a reservation before provider startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveBindTransportRequest {
    /// Reserved module lease.
    pub handle: LiveHandle,
    /// Mechanical host socket/persistence port.
    pub port: LiveTransportPort,
}

/// Advance past the last completely handled frame and obtain the next one.
///
/// The host retains an unacknowledged frame until the exact sequence is
/// acknowledged. It waits briefly for input, returning no frame when idle.
/// It must not race multiple websocket readers or erase a lost reply.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadLiveInputRequest {
    /// Known host transport context.
    pub context_token: String,
    /// Last fully processed frame, zero before any input.
    pub acknowledged_frame: u64,
}

/// A retained client frame or an idle poll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadLiveInputReply {
    /// Next contiguous frame number; idle replies retain the last acknowledged number.
    pub sequence: u64,
    /// None when no client input is available yet.
    pub frame: Option<TransportFrame>,
}

/// Apply these effects in order before acknowledging this delivery.
///
/// The host owns socket writes and transcript persistence. It remembers this
/// delivery ID within the known transport context so retrying a lost reply
/// never duplicates committed persistence or creates another executor.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliverLiveEffectsRequest {
    /// Known host transport context.
    pub context_token: String,
    /// Module lease identifying the delivery namespace.
    pub handle: LiveHandle,
    /// Stable input/output delivery identity within this lease.
    pub delivery_id: String,
    /// Ordered client frames, audio and host persistence effects.
    pub outputs: Vec<LiveOutput>,
}

/// Acknowledgment after all requested effects have been applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverLiveEffectsReply {
    /// False if the known context could not apply the delivery.
    pub delivered: bool,
}

/// Close a known host transport without creating or reviving a context.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseLiveTransportRequest {
    /// Known host transport context.
    pub context_token: String,
}

/// Result of bounded mechanical transport teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseLiveTransportReply {
    /// True for a known already closed context, false for an unknown context.
    pub closed: bool,
}

macro_rules! redacted_debug {
    ($name:ident) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}
redacted_debug!(ReadLiveInputRequest);
redacted_debug!(DeliverLiveEffectsRequest);
redacted_debug!(CloseLiveTransportRequest);
