//! Portable scheduling and completion callbacks for an admitted local-brain turn.

use serde::{Deserialize, Serialize};

/// Mechanical host endpoint for native brain work and product effects.
pub const INTERFACE: &str = "ai.tinyhumans.tinyvoice.RelayHost";
/// Start one known host invocation idempotently.
pub const START_METHOD: &str = "StartRelayBrain";
/// Read selected deltas and completion, replaying the next unacknowledged event.
pub const READ_METHOD: &str = "ReadRelayBrain";
/// Apply one ordered group with a stable idempotency identity.
pub const APPLY_METHOD: &str = "ApplyRelayEffects";
/// Revoke only the known invocation and stop its host-owned work.
pub const FINISH_METHOD: &str = "FinishRelayBrain";

/// Host-selected module scheduling bounds, fixed for one module instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLimits {
    /// Concurrent native brain invocations, between one and sixteen.
    pub max_active_turns: u16,
    /// Total reserved, queued, running or retained turns, up to 256.
    pub max_pending_turns: u16,
}

/// Opaque module reservation, independent of product correlation identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelayHandle {
    /// Unpredictable module-generated identity.
    pub id: String,
}

/// Generic lifecycle facts; no product delivery decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    /// Waiting for a start request.
    Reserved,
    /// Waiting for a native execution slot.
    Queued,
    /// The native brain is running.
    Running,
    /// Spoken acknowledgment completed; native work continues.
    Deferred,
    /// Terminal; explicit release removes the lease.
    Closed,
}

/// State of a known reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayStatus {
    /// Referenced reservation.
    pub handle: RelayHandle,
    /// Module lifecycle state.
    pub state: RelayState,
}

/// Saved host authority; the module cannot recover it from model arguments.
///
/// Before start, bind the reserved module handle to this existing host context.
/// Every callback verifies that exact pair, including queued acknowledgments
/// and cleanup before native execution. Callbacks never register authority from
/// request fields or invent an executor for an unknown context.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayHostPort {
    /// Host bus name.
    pub bus_name: String,
    /// Host object path.
    pub object_path: String,
    /// Existing native turn context, including product correlation and policy.
    pub context_token: String,
}

/// Start generic coordination on an already admitted product turn.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayStartRequest {
    /// Reserved module lease.
    pub handle: RelayHandle,
    /// Saved current native execution authority.
    pub port: RelayHostPort,
    /// Acknowledgment clock starts before waiting for a concurrency slot.
    pub acknowledgment_deadline_ms: u64,
    /// Overall deadline including time spent queued.
    pub turn_timeout_ms: u64,
    /// Host-selected spoken fallback for an empty successful reply.
    pub empty_reply_text: String,
    /// Host-selected spoken acknowledgment when no delta preceded the deadline.
    pub deadline_reply_text: String,
    /// Host-selected fallback when native work disappears without a result.
    pub lost_reply_text: String,
}

/// Release one known turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayHandleRequest {
    /// Referenced module lease.
    pub handle: RelayHandle,
}

/// One known native invocation; repeats must not create another brain.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayInvocationRequest {
    /// Saved known host authority.
    pub context_token: String,
    /// Fresh module identity, separate from product correlation.
    pub invocation_id: String,
}

/// Whether the existing native authority accepted the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRelayBrainReply {
    /// False for revoked or unknown authority; never fabricate an executor.
    pub started: bool,
}

/// Read one retained native event.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRelayBrainRequest {
    /// Existing host authority.
    pub context_token: String,
    /// Known invocation.
    pub invocation_id: String,
    /// Last fully applied event, zero before any event.
    pub acknowledged_event: u64,
}

/// Completion supplied by the host's native local brain.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayOutcome {
    /// Actual successful native reply.
    Success {
        /// Unmodified reply; the host screens delivery.
        text: String,
    },
    /// Actual reported native failure.
    Failure {
        /// Native error, never inferred from a missing result.
        message: String,
    },
    /// Native work disappeared without completing.
    Lost,
}

/// Host-selected speech progress and actual native completion.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayBrainEvent {
    /// Already filtered by current host speech policy.
    Delta {
        /// Selected text; whitespace remains significant.
        text: String,
    },
    /// Only after all preceding selected deltas; no later deltas.
    Complete {
        /// Actual native completion.
        outcome: RelayOutcome,
    },
}

/// An idle poll or the retained next contiguous event.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRelayBrainReply {
    /// Idle polls keep the acknowledged sequence.
    pub sequence: u64,
    /// Hosts return idle promptly; remote call deadlines do not cancel remote work.
    pub event: Option<RelayBrainEvent>,
}

/// Generic effects; the host chooses wire events and product destinations.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayEffect {
    /// Selected or explicitly supplied fallback speech.
    Delta {
        /// Text to forward without discarding whitespace.
        text: String,
    },
    /// End the foreground spoken turn after all preceding deltas.
    Done,
    /// Report a foreground native or coordination failure.
    Error {
        /// Actual native error or a bounded generic coordination error.
        message: String,
    },
    /// Native completion after the foreground deadline.
    Deferred {
        /// Actual outcome; host decides chat/readback eligibility.
        outcome: RelayOutcome,
    },
    /// Native work disappeared; host decides whether a failure notice is warranted.
    LostBrain,
}

/// Apply these ordered effects once even if the acknowledgment reply is lost.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyRelayEffectsRequest {
    /// Existing host turn context.
    pub context_token: String,
    /// Known native invocation.
    pub invocation_id: String,
    /// Stable identity within this invocation.
    pub delivery_id: u64,
    /// Ordered effects; a done never precedes its foreground deltas.
    pub effects: Vec<RelayEffect>,
}

/// Acknowledgment after effects commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyRelayEffectsReply {
    /// False when current authority or delivery is unavailable.
    pub applied: bool,
}

/// Cleanup never creates an unknown invocation or revives a finished one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishRelayBrainReply {
    /// True for a known invocation that has stopped, including repeat cleanup.
    pub finished: bool,
}

macro_rules! redacted_debug {
    ($($name:ident),+ $(,)?) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}
redacted_debug!(
    RelayHostPort,
    RelayStartRequest,
    RelayInvocationRequest,
    ReadRelayBrainRequest,
    RelayOutcome,
    RelayBrainEvent,
    ReadRelayBrainReply,
    RelayEffect,
    ApplyRelayEffectsRequest,
);
