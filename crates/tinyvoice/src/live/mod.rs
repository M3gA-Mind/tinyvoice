//! Portable live frame parsing without sockets, identity or provider policy.

use tinyvoice_bus::live::{
    ClientFrame, LiveTransportAction, PrepareLiveInputRequest, PreparedLiveInput, TransportFrame,
};

use crate::{Error, Result};

/// Parse the initial start frame or a later live command.
///
/// Initial errors preserve the wire-visible diagnostic. Later malformed and
/// duplicate start frames are ignored, matching the established duplex protocol.
/// This function does not validate provider choice, identity or sample rate.
///
/// # Errors
///
/// A missing, malformed or non-start initial text frame.
pub fn prepare_input(request: PrepareLiveInputRequest) -> Result<PreparedLiveInput> {
    if request.awaiting_start {
        let TransportFrame::Text { text } = request.frame else {
            return Err(Error::LiveProtocol(
                "the first frame must be a start frame".into(),
            ));
        };
        let frame = serde_json::from_str::<ClientFrame>(&text)
            .map_err(|error| Error::LiveProtocol(format!("bad start frame: {error}")))?;
        return if matches!(frame, ClientFrame::Start { .. }) {
            Ok(PreparedLiveInput::Control(frame))
        } else {
            Err(Error::LiveProtocol(
                "the first frame must be a start frame".into(),
            ))
        };
    }
    Ok(match request.frame {
        TransportFrame::Text { text } => match serde_json::from_str::<ClientFrame>(&text) {
            Ok(frame @ (ClientFrame::Text { .. } | ClientFrame::Interrupt | ClientFrame::Stop)) => {
                PreparedLiveInput::Control(frame)
            }
            _ => PreparedLiveInput::Transport(LiveTransportAction::Ignore),
        },
        TransportFrame::Audio { pcm16 } => {
            PreparedLiveInput::Transport(LiveTransportAction::Audio { pcm16 })
        }
        TransportFrame::Closed => PreparedLiveInput::Transport(LiveTransportAction::Closed),
        TransportFrame::Other => PreparedLiveInput::Transport(LiveTransportAction::Ignore),
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
