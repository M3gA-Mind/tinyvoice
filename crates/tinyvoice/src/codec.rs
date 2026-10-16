//! In-memory voice-note decoding, encoding, probing, and waveform extraction.
//!
//! The public functions accept byte slices and return owned values. They do no
//! filesystem, device, network, process, or runtime work. Audio is normalized
//! to mono 48 kHz for encoded voice-note output.

use std::io::Cursor;

use opus_pure::{Application, MAX_PACKET_BYTES, MAX_PACKET_SAMPLES, OggOpusReader, OggOpusWriter, OpusEncoder, OpusHead, Trim};
use symphonia::core::{
    formats::{FormatOptions, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};

use crate::{Error, Result, audio};

/// The supported voice-note container and codec combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioCodec {
    /// PCM signed 16-bit little-endian WAV.
    WavPcm16,
    /// Opus audio in an Ogg container.
    OggOpus,
    /// AAC-LC audio in an M4A/ISO-BMFF container.
    M4aAac,
}

/// Container facts obtained by [`probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioInfo {
    /// Detected container and codec.
    pub codec: AudioCodec,
    /// Decoded duration, rounded down to whole milliseconds.
    pub duration_ms: u64,
    /// Decoded audio sample rate in Hz.
    pub sample_rate: u32,
    /// Decoded channel count.
    pub channels: u16,
}

const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_DECODED_SAMPLES: usize = 48_000 * 300;
const VOICE_RATE: u32 = 48_000;
const OPUS_FRAME: usize = 960;
const AAC_FRAME: usize = 1024;
const AAC_BITRATE: u32 = 64_000;

/// Detect a supported input, validate and decode it, then report its facts.
///
/// # Errors
///
/// Returns [`Error::AudioCodec`] for unsupported, corrupt, truncated, or
/// over-limit media.
pub fn probe(bytes: &[u8]) -> Result<AudioInfo> {
    let (codec, samples, rate, channels) = decode(bytes)?;
    let duration_ms = (samples.len() as u64).saturating_mul(1000) / u64::from(rate);
    Ok(AudioInfo { codec, duration_ms, sample_rate: rate, channels })
}

/// Convert supported audio to the requested voice-note format.
///
/// Conversions decode to mono 48 kHz first. Requests already in their target
/// format return the original bytes unchanged.
///
/// # Errors
///
/// Returns [`Error::AudioCodec`] when input or output audio cannot be handled.
pub fn transcode(bytes: &[u8], target: AudioCodec) -> Result<Vec<u8>> {
    let (source, samples, rate, _) = decode(bytes)?;
    if source == target {
        return Ok(bytes.to_vec());
    }
    let samples = if rate == VOICE_RATE { samples } else { audio::resample(&samples, rate, VOICE_RATE)? };
    match target {
        AudioCodec::WavPcm16 => audio::f32_mono_to_wav(&samples, VOICE_RATE),
        AudioCodec::OggOpus => encode_opus(&samples),
        AudioCodec::M4aAac => encode_aac_m4a(&samples),
    }
}

/// Return normalized peak-amplitude bytes for a decoded waveform.
///
/// The output has exactly `buckets` entries. Each entry is a peak amplitude
/// normalized against the clip's loudest bucket, matching the compact byte
/// shape used by voice-note clients.
///
/// # Errors
///
/// Returns [`Error::AudioCodec`] for invalid audio or when `buckets` is zero or
/// exceeds 4096.
pub fn waveform(bytes: &[u8], buckets: usize) -> Result<Vec<u8>> {
    if buckets == 0 || buckets > 4096 {
        return Err(codec_error("waveform bucket count must be between 1 and 4096"));
    }
    let (_, samples, rate, _) = decode(bytes)?;
    let samples = if rate == VOICE_RATE { samples } else { audio::resample(&samples, rate, VOICE_RATE)? };
    if samples.is_empty() {
        return Err(codec_error("audio contains no samples"));
    }
    let mut peaks = Vec::with_capacity(buckets);
    for bucket in 0..buckets {
        let start = bucket * samples.len() / buckets;
        let end = ((bucket + 1) * samples.len() / buckets).max(start + 1).min(samples.len());
        let peak = samples[start..end].iter().fold(0.0f32, |largest, sample| largest.max(sample.abs()));
        peaks.push(peak);
    }
    let max_peak = peaks.iter().copied().fold(0.0f32, f32::max);
    if max_peak == 0.0 {
        return Ok(vec![0; buckets]);
    }
    Ok(peaks.into_iter().map(|peak| (peak / max_peak * 255.0).round() as u8).collect())
}

fn decode(bytes: &[u8]) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        return Err(codec_error("audio input is empty or exceeds the 32 MiB limit"));
    }
    if bytes.starts_with(b"OggS") {
        decode_opus(bytes).map(|(samples, rate, channels)| (AudioCodec::OggOpus, samples, rate, channels))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        decode_symphonia(bytes, AudioCodec::WavPcm16, "wav")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        decode_symphonia(bytes, AudioCodec::M4aAac, "m4a")
    } else {
        Err(codec_error("unsupported audio container"))
    }
}

fn decode_opus(bytes: &[u8]) -> Result<(Vec<f32>, u32, u16)> {
    let mut reader = OggOpusReader::new(Cursor::new(bytes.to_vec())).map_err(codec_error_from)?;
    let head = reader.head().clone();
    let channels = usize::from(head.channel_count);
    if channels == 0 || channels > 2 {
        return Err(codec_error("voice Opus supports one or two channels"));
    }
    let mut decoder = head.decoder(VOICE_RATE as i32).map_err(codec_error_from)?;
    let mut trim = Trim::new(&head, VOICE_RATE as i32, channels).map_err(codec_error_from)?;
    let mut pcm = vec![0.0; MAX_PACKET_SAMPLES * channels];
    let mut mono = Vec::new();
    let mut saw_packet = false;
    let mut saw_eos = false;
    while let Some(packet) = reader.read_packet().map_err(codec_error_from)? {
        saw_packet = true;
        saw_eos |= packet.end_of_stream;
        let count = decoder.decode(&packet.data, MAX_PACKET_SAMPLES, &mut pcm).map_err(codec_error_from)?;
        let trimmed = trim.keep(&packet, &pcm[..count * channels]);
        for frame in trimmed.chunks_exact(channels) {
            mono.push(frame.iter().sum::<f32>() / channels as f32);
        }
        if mono.len() > MAX_DECODED_SAMPLES {
            return Err(codec_error("decoded audio exceeds the five minute limit"));
        }
    }
    if !saw_packet || !saw_eos || mono.is_empty() {
        return Err(codec_error("Ogg Opus stream is truncated or contains no audio"));
    }
    Ok((mono, VOICE_RATE, channels as u16))
}

fn decode_symphonia(bytes: &[u8], codec: AudioCodec, extension: &str) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, source, FormatOptions::default(), MetadataOptions::default())
        .map_err(codec_error_from)?;
    let track = format.default_track(symphonia::core::formats::TrackType::Audio).ok_or_else(|| codec_error("audio stream has no default track"))?;
    let track_id = track.id;
    let codec_params = track.codec_params.as_ref().and_then(|params| params.audio()).ok_or_else(|| codec_error("audio codec parameters are missing"))?.clone();
    let rate = codec_params.sample_rate.ok_or_else(|| codec_error("audio sample rate is missing"))?;
    if rate == 0 {
        return Err(codec_error("audio sample rate is zero"));
    }
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &Default::default())
        .map_err(codec_error_from)?;
    let mut samples = Vec::new();
    let mut channel_count = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => return Err(codec_error_from(error)),
        };
        if packet.track_id != track_id { continue; }
        let decoded = decoder.decode(&packet).map_err(codec_error_from)?;
        let channels = decoded.spec().channels().count();
        if channels == 0 || channels > 8 { return Err(codec_error("audio channel count is unsupported")); }
        channel_count = channels;
        let mut interleaved = vec![0.0f32; decoded.samples_interleaved()];
        decoded.copy_to_slice_interleaved::<f32, _>(&mut interleaved);
        for frame in interleaved.chunks_exact(channels) {
            samples.push(frame.iter().sum::<f32>() / channels as f32);
        }
        if samples.len() > MAX_DECODED_SAMPLES {
            return Err(codec_error("decoded audio exceeds the five minute limit"));
        }
    }
    if samples.is_empty() { return Err(codec_error("audio contains no samples")); }
    Ok((codec, samples, rate, channel_count as u16))
}

fn encode_opus(samples: &[f32]) -> Result<Vec<u8>> {
    if samples.is_empty() { return Err(codec_error("audio contains no samples")); }
    let mut encoder = OpusEncoder::new(VOICE_RATE as i32, 1, Application::Voip).map_err(codec_error_from)?;
    encoder.bitrate_bps = 24_000;
    let head = OpusHead::for_encoder(&encoder, VOICE_RATE);
    let mut writer = OggOpusWriter::new(Vec::new(), head.clone()).map_err(codec_error_from)?;
    let needed_granule = u64::from(head.pre_skip) + samples.len() as u64;
    let packet_count = needed_granule.div_ceil(OPUS_FRAME as u64) as usize;
    let mut packet = vec![0; MAX_PACKET_BYTES];
    for index in 0..packet_count {
        let start = index * OPUS_FRAME;
        let end = (start + OPUS_FRAME).min(samples.len());
        let mut frame = [0.0f32; OPUS_FRAME];
        if start < samples.len() { frame[..end - start].copy_from_slice(&samples[start..end]); }
        let count = encoder.encode(&frame, OPUS_FRAME, &mut packet).map_err(codec_error_from)?;
        let duration = if index + 1 == packet_count {
            u32::try_from(needed_granule.saturating_sub(writer.granule() as u64)).map_err(codec_error_from)?
        } else { OPUS_FRAME as u32 };
        writer.write_packet_with_duration(&packet[..count], duration).map_err(codec_error_from)?;
    }
    writer.finish().map_err(codec_error_from)
}

fn encode_aac_m4a(samples: &[f32]) -> Result<Vec<u8>> {
    use mp4::{AacConfig, AudioObjectType, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, SampleFreqIndex, TrackConfig, TrackType};
    use oxideav_aac::encoder::{EncoderConfig, StreamEncoder};
    use bytes::Bytes;

    if samples.is_empty() { return Err(codec_error("audio contains no samples")); }
    let mut encoder = StreamEncoder::new(EncoderConfig { sample_rate: VOICE_RATE, channels: 1, bitrate: AAC_BITRATE }).map_err(codec_error_from)?;
    let pcm: Vec<i16> = samples.iter().map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect();
    let adts = encoder.encode_all(&pcm).map_err(codec_error_from)?;
    let frames = split_adts(&adts)?;
    if frames.len() < 2 { return Err(codec_error("AAC encoder returned too few frames")); }
    let config = AacConfig { bitrate: AAC_BITRATE, profile: AudioObjectType::AacLowComplexity, freq_index: SampleFreqIndex::Freq48000, chan_conf: ChannelConfig::Mono };
    let track = TrackConfig { track_type: TrackType::Audio, timescale: VOICE_RATE, language: "und".into(), media_conf: MediaConfig::AacConfig(config) };
    let mp4_config = Mp4Config { major_brand: FourCC::from(*b"M4A "), minor_version: 0, compatible_brands: vec![FourCC::from(*b"M4A "), FourCC::from(*b"isom")], timescale: VOICE_RATE };
    let mut writer = Mp4Writer::write_start(Cursor::new(Vec::new()), &mp4_config).map_err(codec_error_from)?;
    writer.add_track(&track).map_err(codec_error_from)?;
    let usable = &frames[1..];
    let mut start_time = 0u64;
    for (index, frame) in usable.iter().enumerate() {
        let duration = if index + 1 == usable.len() { (samples.len() - index * AAC_FRAME).min(AAC_FRAME) as u32 } else { AAC_FRAME as u32 };
        writer.write_sample(1, &Mp4Sample { start_time, duration, rendering_offset: 0, is_sync: true, bytes: Bytes::copy_from_slice(frame) }).map_err(codec_error_from)?;
        start_time += u64::from(duration);
    }
    writer.write_end().map_err(codec_error_from)?;
    Ok(writer.into_writer().into_inner())
}

fn split_adts(bytes: &[u8]) -> Result<Vec<&[u8]>> {
    let mut frames = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if bytes.len() - position < 7 || bytes[position] != 0xff || bytes[position + 1] & 0xf6 != 0xf0 {
            return Err(codec_error("AAC encoder returned invalid ADTS framing"));
        }
        let header = if bytes[position + 1] & 1 == 0 { 9 } else { 7 };
        let length = (usize::from(bytes[position + 3] & 3) << 11) | (usize::from(bytes[position + 4]) << 3) | usize::from(bytes[position + 5] >> 5);
        if length < header || position + length > bytes.len() { return Err(codec_error("AAC encoder returned a truncated ADTS frame")); }
        frames.push(&bytes[position + header..position + length]);
        position += length;
    }
    Ok(frames)
}

fn codec_error(message: impl Into<String>) -> Error { Error::AudioCodec(message.into()) }
fn codec_error_from(error: impl std::fmt::Display) -> Error { codec_error(error.to_string()) }

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
