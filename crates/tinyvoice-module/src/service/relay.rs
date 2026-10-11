//! Bounded relay preparation at the module's serialized bus boundary.

use std::io::{self, Write};

use serde::Serialize;
use tinyvoice_bus::relay::{MAX_RELAY_REQUEST_BYTES, PreparedRelayTurn, RelayTurnRequest};

/// Count serialized bytes without allocating a second copy of caller text.
struct SizeLimit {
    remaining: usize,
}

impl Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining = self.remaining.checked_sub(bytes.len()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "serialized relay payload exceeds limit",
            )
        })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn fits(value: &impl Serialize, limit: usize) -> bool {
    serde_json::to_writer(SizeLimit { remaining: limit }, value).is_ok()
}

/// Reject aggregate input before its text is copied into multiple reply fields.
pub(super) fn prepare(request: &RelayTurnRequest) -> tinybus::Result<PreparedRelayTurn> {
    if !fits(request, MAX_RELAY_REQUEST_BYTES) {
        return Err(tinybus::Error::failed(
            "relay request exceeds the four MiB serialized limit",
        ));
    }
    let reply = tinyvoice::relay::prepare_turn(request);
    // Keep envelope space and defend against future reply fields increasing
    // amplification. The input bound already limits construction allocations.
    if !fits(&reply, tinybus::message::codec::MAX_FRAME_LEN - 64 * 1024) {
        return Err(tinybus::Error::failed(
            "prepared relay reply exceeds the bus frame limit",
        ));
    }
    Ok(reply)
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
