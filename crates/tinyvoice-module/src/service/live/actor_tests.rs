//! Bounded provider text and host cleanup regressions.

use super::*;

#[test]
fn oversized_provider_errors_are_rejected_before_event_projection() {
    let error = Error::Provider("x".repeat(MAX_TEXT_BYTES + 1));
    assert!(oversized(&LiveAgentEvent::Live(LiveEvent::Error {
        error: error.clone(),
        fatal: false
    })));
    assert!(oversized(&LiveAgentEvent::Live(LiveEvent::Closed(
        CloseReason::Error(error)
    ))));
    assert!(oversized(&LiveAgentEvent::Live(LiveEvent::Closed(
        CloseReason::Remote {
            code: Some(1000),
            reason: "x".repeat(MAX_TEXT_BYTES + 1)
        }
    ))));
}
