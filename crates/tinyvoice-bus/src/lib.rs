//! Every type that crosses `TinyVoice`'s `TinyBus` boundary, and the names of
//! the members that carry them.
//!
//! A host loads the `tinyvoice-module` dynamic library but cannot import Rust
//! items from that binary. This transport-free crate is the ordinary library
//! that supplies its call vocabulary: interface names, request and response
//! types, and the compatibility rule for that vocabulary.
//!
//! It deliberately contains no `TinyBus` transport, runtime, or voice-processing
//! behavior. Hosts own their connection and policies; `tinyvoice-module` owns
//! the adapter; and the root `tinyvoice` crate remains host-agnostic.

mod error;

pub mod capture;
pub mod hotkey;
pub mod intent;
pub mod live;
pub mod names;
pub mod provider;
pub mod relay;
pub mod session;
pub mod transcript;
pub mod vad;
pub mod version;

pub use error::{Error, Result};
pub use hotkey::*;
pub use intent::VoiceIntent;
pub use names::{BUS_NAME, METHODS, OBJECT_PATH};
pub use provider::{
    CloudVoiceProvider, ProviderFuture, RealtimeRequest, RealtimeSession, SynthesisRequest,
    Transcription, TranscriptionRequest,
};
pub use session::{
    GateVerdict, RecordingGates, SessionAction, SessionAdvance, SessionInput, SessionPhase,
    SessionStatus, UtteranceDisposition,
};
pub use transcript::Mode;
pub use vad::{IndexedVadEvent, VadConfig, VadEvent};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod test;
pub use version::{CONTRACT_VERSION, is_compatible};
