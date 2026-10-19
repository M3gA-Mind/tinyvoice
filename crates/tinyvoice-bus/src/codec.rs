//! Typed, transport-neutral requests for voice-note codec operations.

use serde::{Deserialize, Serialize};

/// Supported encoded voice-note media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    /// Signed 16-bit little-endian PCM in WAV.
    WavPcm16,
    /// Opus audio in Ogg.
    OggOpus,
    /// AAC-LC audio in M4A.
    M4aAac,
    /// MPEG Layer III audio (source format).
    Mp3,
    /// Adaptive Multi-Rate narrowband audio in its storage format.
    AmrNb,
}

/// Input bytes encoded as standard Base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecInput {
    /// Base64-encoded complete media file.
    pub audio_base64: String,
}

/// Convert media to the requested voice-note codec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecTranscodeRequest {
    /// Base64-encoded source media.
    pub audio_base64: String,
    /// Declared source container and codec, checked against the payload.
    pub from: AudioCodec,
    /// Destination container and codec.
    pub target: AudioCodec,
}

/// Encoded media returned from a transcode operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecOutput {
    /// Base64-encoded complete media file.
    pub audio_base64: String,
}

/// Audio properties decoded from a media file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecInfo {
    /// Detected codec.
    pub codec: AudioCodec,
    /// Duration rounded down to whole milliseconds.
    pub duration_ms: u64,
    /// Original sample rate in Hz.
    pub sample_rate: u32,
    /// Original audio channel count.
    pub channels: u16,
}

/// Extract a fixed-size normalized waveform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformRequest {
    /// Base64-encoded source media.
    pub audio_base64: String,
    /// Requested number of buckets, limited to 1 through 4096.
    pub buckets: usize,
}

/// Normalized peak amplitude bytes, one per requested bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveformResult {
    /// Peak values in the range 0 through 255.
    pub peaks: Vec<u8>,
}

