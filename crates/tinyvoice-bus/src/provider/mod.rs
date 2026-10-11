//! Host-supplied cloud voice transport, independent of native module behavior.
//!
//! A host implements this interface in its backend layer and installs it through
//! its own composition seam. Requests contain voice inputs, never backend URLs,
//! credentials, or product configuration. The implementation owns authentication,
//! endpoint selection and network I/O. Capture, VAD, hotkeys and other module
//! operations continue to use the compiled module; these traits do not replace
//! its execution path or introduce additional `TinyBus` methods.

use std::{future::Future, pin::Pin};

use serde_json::Value;

use crate::Result;

mod types;

pub use types::{
    RealtimeRequest, RealtimeSession, SynthesisRequest, Transcription, TranscriptionRequest,
};

/// A backend operation returning the crate-wide result with a classified error.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Cloud transport supplied by the host's backend layer.
///
/// Implementations own HTTP and credential lifecycle. The consuming voice
/// domain owns provider selection, local fallbacks and presentation. Speech
/// responses remain opaque JSON so existing response normalization can retain
/// its tolerant backend shape without relocating generic voice behavior here.
pub trait CloudVoiceProvider: Send + Sync {
    /// Transcribe encoded audio.
    ///
    /// # Errors
    /// Returns the host-classified input, authentication or transport error.
    fn transcribe(&self, request: TranscriptionRequest) -> ProviderFuture<'_, Transcription>;

    /// Synthesize reply audio and return the provider's response payload.
    ///
    /// # Errors
    /// Returns the host-classified input, authentication or transport error.
    fn synthesize(&self, request: SynthesisRequest) -> ProviderFuture<'_, Value>;

    /// Bootstrap a hosted realtime session.
    ///
    /// # Errors
    /// Returns the host-classified authentication, transport or response error.
    fn realtime(&self, request: RealtimeRequest) -> ProviderFuture<'_, RealtimeSession>;
}
