//! In-memory voice-note decoding, encoding, probing, and waveform extraction.
//!
//! The public functions accept byte slices and return owned values. They do no
//! filesystem, device, network, process, or runtime work. Audio is normalized
//! to mono 48 kHz for Opus/AAC output and 8 kHz for AMR-NB output.

use std::io::Cursor;

use opus_pure::{
    Application, MAX_PACKET_BYTES, MAX_PACKET_SAMPLES, OggOpusReader, OggOpusWriter, OpusEncoder,
    OpusHead, Trim,
};
use symphonia::core::{
    codecs::audio::AudioDecoder,
    formats::{FormatOptions, FormatReader, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};

#[cfg(feature = "amr-nb")]
use codec_core::{
    codecs::amr::{
        AmrCodec, AmrFrameType, AmrMode, AmrPayloadFrame, AmrStorageReader, AmrVariant, storage,
    },
    types::{CodecConfig, CodecType, CodedFrame, FrameKind, VariableRateCodec},
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
    /// MPEG Layer III audio (typically returned by text-to-speech services).
    Mp3,
    /// AMR-NB audio in its storage format, used by `DingTalk` voice messages.
    AmrNb,
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
const MAX_DECODED_SAMPLES: usize = 32_000_000;
const MAX_DURATION_SECONDS: u64 = 300;
const AMR_NB_RATE: u32 = 8_000;
#[cfg(feature = "amr-nb")]
const AMR_NB_FRAME_SAMPLES: usize = 160;
#[cfg(feature = "amr-nb")]
const MAX_AMR_NB_FRAMES: usize = 15_000;
const VOICE_RATE: u32 = 48_000;
const OPUS_FRAME: usize = 960;
const AAC_FRAME_SAMPLES: usize = 1024;
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
    Ok(AudioInfo {
        codec,
        duration_ms,
        sample_rate: rate,
        channels,
    })
}

/// Convert bytes in `from` format to the requested voice-note format.
///
/// The declared source format is checked against the encoded input. Encoded
/// Opus/AAC output is mono 48 kHz; AMR-NB output is mono 8 kHz in 20 ms
/// frames. WAV-to-WAV requests preserve the original bytes. MP3 is accepted as
/// a source format but is not an encoding target.
///
/// # Errors
///
/// Returns [`Error::AudioCodec`] when input or output audio cannot be handled.
pub fn transcode(bytes: &[u8], from: AudioCodec, target: AudioCodec) -> Result<Vec<u8>> {
    let (source, samples, rate, _) = decode_as(bytes, from)?;
    if source == target && source == AudioCodec::WavPcm16 {
        return Ok(bytes.to_vec());
    }
    let target_rate = if target == AudioCodec::AmrNb {
        AMR_NB_RATE
    } else {
        VOICE_RATE
    };
    let samples = if rate == target_rate {
        samples
    } else {
        audio::resample(&samples, rate, target_rate)?
    };
    match target {
        AudioCodec::WavPcm16 => audio::f32_mono_to_wav(&samples, VOICE_RATE),
        AudioCodec::OggOpus => encode_opus(&samples),
        AudioCodec::M4aAac => encode_aac_m4a(&samples),
        AudioCodec::Mp3 => Err(codec_error("MP3 encoding is not supported")),
        AudioCodec::AmrNb => encode_amr_nb(&samples),
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
        return Err(codec_error(
            "waveform bucket count must be between 1 and 4096",
        ));
    }
    let (_, samples, rate, _) = decode(bytes)?;
    let samples = if rate == VOICE_RATE {
        samples
    } else {
        audio::resample(&samples, rate, VOICE_RATE)?
    };
    if samples.is_empty() {
        return Err(codec_error("audio contains no samples"));
    }
    let mut peaks = Vec::with_capacity(buckets);
    for bucket in 0..buckets {
        let (start, end) = waveform_bucket_range(samples.len(), buckets, bucket)?;
        let peak = samples[start..end]
            .iter()
            .fold(0.0f32, |largest, sample| largest.max(sample.abs()));
        peaks.push(peak);
    }
    let max_peak = peaks.iter().copied().fold(0.0f32, f32::max);
    if max_peak == 0.0 {
        return Ok(vec![0; buckets]);
    }
    Ok(peaks
        .into_iter()
        .map(|peak| normalized_peak_byte(peak / max_peak))
        .collect())
}

fn waveform_bucket_range(
    sample_count: usize,
    bucket_count: usize,
    index: usize,
) -> Result<(usize, usize)> {
    let sample_count_u128 = u128::try_from(sample_count).map_err(codec_error_from)?;
    let bucket_count_u128 = u128::try_from(bucket_count).map_err(codec_error_from)?;
    let index_u128 = u128::try_from(index).map_err(codec_error_from)?;
    let start = usize::try_from(index_u128 * sample_count_u128 / bucket_count_u128)
        .map_err(codec_error_from)?;
    let end = usize::try_from((index_u128 + 1) * sample_count_u128 / bucket_count_u128)
        .map_err(codec_error_from)?;
    Ok((start, end.max(start.saturating_add(1)).min(sample_count)))
}

/// Convert a known finite amplitude ratio in `[0, 1]` to its byte scale.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn normalized_peak_byte(ratio: f32) -> u8 {
    (ratio.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn decode(bytes: &[u8]) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        return Err(codec_error(
            "audio input is empty or exceeds the 32 MiB limit",
        ));
    }
    if bytes.starts_with(b"OggS") {
        decode_opus(bytes)
            .map(|(samples, rate, channels)| (AudioCodec::OggOpus, samples, rate, channels))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        decode_symphonia(bytes, AudioCodec::WavPcm16, "wav")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        decode_symphonia(bytes, AudioCodec::M4aAac, "m4a")
    } else if looks_like_mp3(bytes) {
        decode_symphonia(bytes, AudioCodec::Mp3, "mp3")
    } else if bytes.starts_with(b"#!AMR\n") {
        decode_amr_nb(bytes)
    } else {
        Err(codec_error("unsupported audio container"))
    }
}

fn decode_as(bytes: &[u8], expected: AudioCodec) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        return Err(codec_error(
            "audio input is empty or exceeds the 32 MiB limit",
        ));
    }
    let decoded = match expected {
        AudioCodec::OggOpus if bytes.starts_with(b"OggS") => decode_opus(bytes)
            .map(|(samples, rate, channels)| (AudioCodec::OggOpus, samples, rate, channels)),
        AudioCodec::WavPcm16 if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") => {
            decode_symphonia(bytes, AudioCodec::WavPcm16, "wav")
        }
        AudioCodec::M4aAac if bytes.get(4..8) == Some(b"ftyp") => {
            decode_symphonia(bytes, AudioCodec::M4aAac, "m4a")
        }
        AudioCodec::Mp3 if looks_like_mp3(bytes) => decode_symphonia(bytes, AudioCodec::Mp3, "mp3"),
        AudioCodec::AmrNb if bytes.starts_with(b"#!AMR\n") => decode_amr_nb(bytes),
        _ => {
            return Err(codec_error(
                "audio input does not match the declared source codec",
            ));
        }
    }?;
    Ok(decoded)
}

fn looks_like_mp3(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ID3")
        || bytes
            .get(..2)
            .is_some_and(|header| header[0] == 0xff && header[1] & 0xe0 == 0xe0)
}

#[cfg(feature = "amr-nb")]
fn encode_amr_nb(samples: &[f32]) -> Result<Vec<u8>> {
    if samples.is_empty() {
        return Err(codec_error("audio contains no samples"));
    }
    let frame_count = samples.len().div_ceil(AMR_NB_FRAME_SAMPLES);
    if frame_count > MAX_AMR_NB_FRAMES {
        return Err(codec_error("AMR-NB audio exceeds the five minute limit"));
    }

    let mut amr_codec =
        AmrCodec::new(&CodecConfig::new(CodecType::AmrNb)).map_err(codec_error_from)?;
    let mut frames = Vec::with_capacity(frame_count);
    for chunk in samples.chunks(AMR_NB_FRAME_SAMPLES) {
        let mut pcm = [0i16; AMR_NB_FRAME_SAMPLES];
        for (destination, source) in pcm.iter_mut().zip(chunk) {
            *destination = pcm16_sample(*source);
        }
        let coded_frame = amr_codec.encode_frame(&pcm).map_err(codec_error_from)?;
        let frame_type = match coded_frame.kind {
            FrameKind::Speech => AmrFrameType::Speech(
                AmrMode::new(AmrVariant::NarrowBand, coded_frame.mode).map_err(codec_error_from)?,
            ),
            FrameKind::ComfortNoise => AmrFrameType::Sid(AmrVariant::NarrowBand),
            FrameKind::NoData => AmrFrameType::NoData,
            FrameKind::Lost => return Err(codec_error("AMR-NB encoder returned a lost frame")),
        };
        frames.push(
            AmrPayloadFrame::new(frame_type, coded_frame.quality_ok, coded_frame.data)
                .map_err(codec_error_from)?,
        );
    }
    storage::write(AmrVariant::NarrowBand, &frames).map_err(codec_error_from)
}

#[cfg(not(feature = "amr-nb"))]
fn encode_amr_nb(_samples: &[f32]) -> Result<Vec<u8>> {
    Err(codec_error("AMR-NB codec support is not enabled"))
}

#[cfg(feature = "amr-nb")]
fn decode_amr_nb(bytes: &[u8]) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    let mut validation = AmrStorageReader::new(bytes).map_err(codec_error_from)?;
    if validation.variant() != AmrVariant::NarrowBand {
        return Err(codec_error("AMR-WB storage is not supported"));
    }
    let mut frame_count = 0usize;
    while validation.next_frame().map_err(codec_error_from)?.is_some() {
        frame_count = frame_count
            .checked_add(1)
            .ok_or_else(|| codec_error("AMR-NB frame count overflow"))?;
        if frame_count > MAX_AMR_NB_FRAMES {
            return Err(codec_error("AMR-NB audio exceeds the five minute limit"));
        }
    }
    if frame_count == 0 {
        return Err(codec_error("AMR-NB file contains no audio frames"));
    }

    let mut reader = AmrStorageReader::new(bytes).map_err(codec_error_from)?;
    let mut amr_codec =
        AmrCodec::new(&CodecConfig::new(CodecType::AmrNb)).map_err(codec_error_from)?;
    let mut samples = Vec::with_capacity(frame_count * AMR_NB_FRAME_SAMPLES);
    while let Some(frame) = reader.next_frame().map_err(codec_error_from)? {
        let coded_frame = match frame.frame_type {
            AmrFrameType::Speech(mode) => CodedFrame::speech(mode.index(), frame.data),
            AmrFrameType::Sid(AmrVariant::NarrowBand) => CodedFrame::comfort_noise(frame.data),
            AmrFrameType::NoData => CodedFrame::no_data(),
            AmrFrameType::SpeechLost | AmrFrameType::Sid(AmrVariant::WideBand) => {
                return Err(codec_error("AMR storage frame does not match AMR-NB"));
            }
        };
        let coded_frame = if frame.quality_ok {
            coded_frame
        } else {
            coded_frame.with_bad_quality()
        };
        let decoded = amr_codec
            .decode_frame(&coded_frame)
            .map_err(codec_error_from)?;
        if decoded.len() != AMR_NB_FRAME_SAMPLES {
            return Err(codec_error("AMR-NB decoder returned an invalid frame size"));
        }
        samples.extend(
            decoded
                .into_iter()
                .map(|sample| f32::from(sample) / 32_768.0),
        );
    }
    Ok((AudioCodec::AmrNb, samples, AMR_NB_RATE, 1))
}

#[cfg(not(feature = "amr-nb"))]
fn decode_amr_nb(_bytes: &[u8]) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    Err(codec_error("AMR-NB codec support is not enabled"))
}

fn decode_opus(bytes: &[u8]) -> Result<(Vec<f32>, u32, u16)> {
    let mut reader = OggOpusReader::new(Cursor::new(bytes.to_vec())).map_err(codec_error_from)?;
    let head = reader.head().clone();
    let channels = usize::from(head.channel_count);
    if channels == 0 || channels > 2 {
        return Err(codec_error("voice Opus supports one or two channels"));
    }
    let mut decoder = head.decoder(48_000).map_err(codec_error_from)?;
    let mut trim = Trim::new(&head, 48_000, channels).map_err(codec_error_from)?;
    let mut pcm = vec![0.0; MAX_PACKET_SAMPLES * channels];
    let mut mono = Vec::new();
    let mut saw_packet = false;
    let mut saw_eos = false;
    while let Some(packet) = reader.read_packet().map_err(codec_error_from)? {
        saw_packet = true;
        saw_eos |= packet.end_of_stream;
        let count = decoder
            .decode(&packet.data, MAX_PACKET_SAMPLES, &mut pcm)
            .map_err(codec_error_from)?;
        let trimmed = trim.keep(&packet, &pcm[..count * channels]);
        for frame in trimmed.chunks_exact(channels) {
            mono.push(frame.iter().sum::<f32>() / f32::from(head.channel_count));
        }
        if !within_duration(mono.len(), VOICE_RATE) {
            return Err(codec_error("decoded audio exceeds the five minute limit"));
        }
    }
    if !saw_packet || !saw_eos || mono.is_empty() {
        return Err(codec_error(
            "Ogg Opus stream is truncated or contains no audio",
        ));
    }
    Ok((
        mono,
        VOICE_RATE,
        u16::try_from(channels).map_err(codec_error_from)?,
    ))
}

fn decode_symphonia(
    bytes: &[u8],
    codec: AudioCodec,
    extension: &str,
) -> Result<(AudioCodec, Vec<f32>, u32, u16)> {
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let source = MediaSourceStream::new(
        Box::new(Cursor::new(bytes.to_vec())),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| codec_error(format!("audio container probe failed: {error}")))?;
    let track = format
        .default_track(symphonia::core::formats::TrackType::Audio)
        .ok_or_else(|| codec_error("audio stream has no default track"))?;
    let track_id = track.id;
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| codec_error("audio codec parameters are missing"))?
        .clone();
    let expected_codec = match codec {
        AudioCodec::WavPcm16 => symphonia::core::codecs::audio::well_known::CODEC_ID_PCM_S16LE,
        AudioCodec::M4aAac => symphonia::core::codecs::audio::well_known::CODEC_ID_AAC,
        AudioCodec::Mp3 => symphonia::core::codecs::audio::well_known::CODEC_ID_MP3,
        AudioCodec::OggOpus => return Err(codec_error("Ogg Opus uses its dedicated decoder")),
        AudioCodec::AmrNb => return Err(codec_error("AMR-NB uses its dedicated decoder")),
    };
    if codec_params.codec != expected_codec {
        return Err(codec_error(
            "audio stream does not match its declared codec",
        ));
    }
    let rate = codec_params
        .sample_rate
        .ok_or_else(|| codec_error("audio sample rate is missing"))?;
    if !(8_000..=192_000).contains(&rate) {
        return Err(codec_error(
            "audio sample rate must be between 8 and 192 kHz",
        ));
    }
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(
            &codec_params,
            &symphonia::core::codecs::audio::AudioDecoderOptions::default(),
        )
        .map_err(codec_error_from)?;
    let decoder_padding = if codec == AudioCodec::M4aAac {
        AAC_FRAME_SAMPLES
    } else {
        0
    };
    let (mut samples, channel_count) =
        decode_packets(&mut *format, track_id, &mut *decoder, rate, decoder_padding)?;
    if samples.is_empty() {
        return Err(codec_error("audio contains no samples"));
    }
    if codec == AudioCodec::M4aAac {
        let mp4 = mp4::Mp4Reader::read_header(Cursor::new(bytes.to_vec()), bytes.len() as u64)
            .map_err(codec_error_from)?;
        let playable_samples = m4a_playable_samples(&mp4, track_id, rate, samples.len())?;
        if !within_duration(playable_samples, rate) || playable_samples > MAX_DECODED_SAMPLES {
            return Err(codec_error("decoded audio exceeds the five minute limit"));
        }
        samples.truncate(playable_samples);
        if samples.is_empty() {
            return Err(codec_error("M4A audio duration is empty"));
        }
    }
    Ok((
        codec,
        samples,
        rate,
        u16::try_from(channel_count).map_err(codec_error_from)?,
    ))
}

fn m4a_playable_samples(
    mp4: &mp4::Mp4Reader<Cursor<Vec<u8>>>,
    track_id: u32,
    rate: u32,
    decoded_samples: usize,
) -> Result<usize> {
    let audio_track = mp4
        .tracks()
        .get(&track_id)
        .filter(|track| {
            track
                .track_type()
                .is_ok_and(|kind| kind == mp4::TrackType::Audio)
        })
        .ok_or_else(|| codec_error("decoded M4A audio track metadata is missing"))?;
    let edit_list = audio_track
        .trak
        .edts
        .as_ref()
        .and_then(|edts| edts.elst.as_ref());
    let (duration_units, duration_timescale) = if let Some(elst) = edit_list {
        let duration_units = elst
            .entries
            .iter()
            .try_fold(0u64, |total, entry| {
                total.checked_add(entry.segment_duration)
            })
            .ok_or_else(|| codec_error("M4A edit list duration overflows"))?;
        (duration_units, mp4.timescale())
    } else {
        (
            audio_track.trak.mdia.mdhd.duration,
            audio_track.trak.mdia.mdhd.timescale,
        )
    };
    if duration_timescale == 0
        || u128::from(duration_units)
            > u128::from(MAX_DURATION_SECONDS) * u128::from(duration_timescale)
    {
        return Err(codec_error("M4A duration exceeds the five minute limit"));
    }
    Ok(
        (u128::from(duration_units) * u128::from(rate) / u128::from(duration_timescale))
            .min(decoded_samples as u128) as usize,
    )
}

fn decode_packets(
    format: &mut dyn FormatReader,
    track_id: u32,
    decoder: &mut dyn AudioDecoder,
    rate: u32,
    decoder_padding: usize,
) -> Result<(Vec<f32>, usize)> {
    let mut samples = Vec::new();
    let mut channel_count = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => {
                return Err(codec_error(format!(
                    "audio container packet read failed: {error}"
                )));
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio_buffer = decoder
            .decode(&packet)
            .map_err(|error| codec_error(format!("audio packet decode failed: {error}")))?;
        let channels = audio_buffer.spec().channels().count();
        if channels == 0 || channels > 8 {
            return Err(codec_error("audio channel count is unsupported"));
        }
        channel_count = channels;
        let mut interleaved = vec![0.0f32; audio_buffer.samples_interleaved()];
        audio_buffer.copy_to_slice_interleaved::<f32, _>(&mut interleaved);
        for frame in interleaved.chunks_exact(channels) {
            samples.push(
                frame.iter().sum::<f32>()
                    / f32::from(u8::try_from(channels).map_err(codec_error_from)?),
            );
        }
        if !within_duration_with_padding(samples.len(), rate, decoder_padding)
            || samples.len() > MAX_DECODED_SAMPLES
        {
            return Err(codec_error("decoded audio exceeds the five minute limit"));
        }
    }
    Ok((samples, channel_count))
}

fn encode_opus(samples: &[f32]) -> Result<Vec<u8>> {
    if samples.is_empty() {
        return Err(codec_error("audio contains no samples"));
    }
    let mut encoder = OpusEncoder::new(48_000, 1, Application::Voip).map_err(codec_error_from)?;
    encoder.bitrate_bps = 24_000;
    let head = OpusHead::for_encoder(&encoder, VOICE_RATE);
    let mut writer = OggOpusWriter::new(Vec::new(), head.clone()).map_err(codec_error_from)?;
    let needed_granule = u64::from(head.pre_skip) + samples.len() as u64;
    let packet_count = usize::try_from(needed_granule.div_ceil(960)).map_err(codec_error_from)?;
    let mut packet = vec![0; MAX_PACKET_BYTES];
    for index in 0..packet_count {
        let start = index * OPUS_FRAME;
        let end = (start + OPUS_FRAME).min(samples.len());
        let mut frame = [0.0f32; OPUS_FRAME];
        if start < samples.len() {
            frame[..end - start].copy_from_slice(&samples[start..end]);
        }
        let count = encoder
            .encode(&frame, OPUS_FRAME, &mut packet)
            .map_err(codec_error_from)?;
        let duration = if index + 1 == packet_count {
            u32::try_from(needed_granule.saturating_sub(writer.granule().cast_unsigned()))
                .map_err(codec_error_from)?
        } else {
            960
        };
        writer
            .write_packet_with_duration(&packet[..count], duration)
            .map_err(codec_error_from)?;
    }
    writer.finish().map_err(codec_error_from)
}

fn encode_aac_m4a(samples: &[f32]) -> Result<Vec<u8>> {
    use bytes::Bytes;
    use mp4::{
        AacConfig, AudioObjectType, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample,
        Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
    };
    use oxideav_aac::encoder::{EncoderConfig, StreamEncoder};

    if samples.is_empty() {
        return Err(codec_error("audio contains no samples"));
    }
    let mut encoder = StreamEncoder::new(EncoderConfig {
        sample_rate: VOICE_RATE,
        channels: 1,
        bitrate: AAC_BITRATE,
    })
    .map_err(codec_error_from)?;
    let pcm: Vec<i16> = samples.iter().map(|sample| pcm16_sample(*sample)).collect();
    let adts = encoder.encode_all(&pcm).map_err(codec_error_from)?;
    let frames = split_adts(&adts)?;
    if frames.len() < 2 {
        return Err(codec_error("AAC encoder returned too few frames"));
    }
    let config = AacConfig {
        bitrate: AAC_BITRATE,
        profile: AudioObjectType::AacLowComplexity,
        freq_index: SampleFreqIndex::Freq48000,
        chan_conf: ChannelConfig::Mono,
    };
    let track = TrackConfig {
        track_type: TrackType::Audio,
        timescale: VOICE_RATE,
        language: "und".into(),
        media_conf: MediaConfig::AacConfig(config),
    };
    let mp4_config = Mp4Config {
        major_brand: FourCC::from(*b"M4A "),
        minor_version: 0,
        compatible_brands: vec![FourCC::from(*b"M4A "), FourCC::from(*b"isom")],
        timescale: VOICE_RATE,
    };
    let mut writer =
        Mp4Writer::write_start(Cursor::new(Vec::new()), &mp4_config).map_err(codec_error_from)?;
    writer.add_track(&track).map_err(codec_error_from)?;
    let usable = &frames[1..];
    let mut start_time = 0u64;
    for frame in usable {
        let duration = 1024;
        writer
            .write_sample(
                1,
                &Mp4Sample {
                    start_time,
                    duration,
                    rendering_offset: 0,
                    is_sync: true,
                    bytes: Bytes::copy_from_slice(frame),
                },
            )
            .map_err(codec_error_from)?;
        start_time += u64::from(duration);
    }
    writer.write_end().map_err(codec_error_from)?;
    repair_m4a(writer.into_writer().into_inner(), samples.len())
}

/// Repair the MP4 muxer's empty `SLConfig` descriptor and add a gapless edit.
///
/// `mp4` 0.14 writes an empty-valued `SLConfig` descriptor placeholder, while
/// ISO/IEC 14496-1 requires its one-byte predefined value. Fill that existing
/// byte without changing atom sizes. The edit list clips AAC's 1024-sample
/// frame padding to the original PCM duration.
#[allow(clippy::too_many_lines)]
fn repair_m4a(mut bytes: Vec<u8>, duration_samples: usize) -> Result<Vec<u8>> {
    let esds_type = find_last(&bytes, b"esds")
        .ok_or_else(|| codec_error("M4A muxer omitted the AAC decoder configuration"))?;
    let esds_start = esds_type
        .checked_sub(4)
        .ok_or_else(|| codec_error("invalid M4A AAC decoder configuration"))?;
    let descriptor_end = esds_start
        .checked_add(read_be_u32(&bytes, esds_start)? as usize)
        .ok_or_else(|| codec_error("M4A AAC decoder configuration size overflow"))?;
    let descriptor_start = esds_type
        .checked_add(8)
        .ok_or_else(|| codec_error("M4A descriptor offset overflow"))?;
    let descriptors = bytes
        .get(descriptor_start..descriptor_end)
        .ok_or_else(|| codec_error("truncated M4A AAC descriptors"))?;
    let sl_config = descriptors
        .windows(3)
        .rposition(|window| window == [0x06, 0x00, 0x00])
        .and_then(|offset| descriptor_start.checked_add(offset))
        .ok_or_else(|| codec_error("M4A AAC decoder configuration has no SLConfig descriptor"))?;
    *bytes
        .get_mut(sl_config + 1)
        .ok_or_else(|| codec_error("truncated M4A SLConfig descriptor"))? = 1;
    *bytes
        .get_mut(sl_config + 2)
        .ok_or_else(|| codec_error("truncated M4A SLConfig predefined value"))? = 2;

    let mdia_type =
        find_last(&bytes, b"mdia").ok_or_else(|| codec_error("M4A media atom is missing"))?;
    let trak_type = find_last(&bytes[..mdia_type], b"trak")
        .ok_or_else(|| codec_error("M4A track atom is missing"))?;
    let movie_type =
        find_last(&bytes, b"moov").ok_or_else(|| codec_error("M4A movie atom is missing"))?;
    let edit = edit_list_atom(duration_samples)?;
    let edit_len = u32::try_from(edit.len()).map_err(codec_error_from)?;
    for atom_type in [b"trak".as_slice(), b"moov"] {
        let position = if atom_type == b"trak" {
            trak_type
        } else {
            movie_type
        };
        let size_position = position
            .checked_sub(4)
            .ok_or_else(|| codec_error("invalid M4A atom position"))?;
        let size = read_be_u32(&bytes, size_position)?;
        let size_end = size_position
            .checked_add(4)
            .ok_or_else(|| codec_error("M4A atom position overflow"))?;
        bytes
            .get_mut(size_position..size_end)
            .ok_or_else(|| codec_error("truncated M4A atom size"))?
            .copy_from_slice(
                &size
                    .checked_add(edit_len)
                    .ok_or_else(|| codec_error("M4A atom is too large"))?
                    .to_be_bytes(),
            );
    }
    let tkhd_type = find_last(&bytes[..mdia_type], b"tkhd")
        .ok_or_else(|| codec_error("M4A track header is missing"))?;
    let tkhd_duration = tkhd_type
        .checked_add(24)
        .ok_or_else(|| codec_error("M4A track header offset overflow"))?;
    let tkhd_end = tkhd_duration
        .checked_add(4)
        .ok_or_else(|| codec_error("M4A track header offset overflow"))?;
    bytes
        .get_mut(tkhd_duration..tkhd_end)
        .ok_or_else(|| codec_error("truncated M4A track header"))?
        .copy_from_slice(
            &u32::try_from(duration_samples)
                .map_err(codec_error_from)?
                .to_be_bytes(),
        );
    let mvhd_type = find_last(&bytes[..mdia_type], b"mvhd")
        .ok_or_else(|| codec_error("M4A movie header is missing"))?;
    let mvhd_duration = mvhd_type
        .checked_add(20)
        .ok_or_else(|| codec_error("M4A movie header offset overflow"))?;
    let mvhd_end = mvhd_duration
        .checked_add(4)
        .ok_or_else(|| codec_error("M4A movie header offset overflow"))?;
    bytes
        .get_mut(mvhd_duration..mvhd_end)
        .ok_or_else(|| codec_error("truncated M4A movie header"))?
        .copy_from_slice(
            &u32::try_from(duration_samples)
                .map_err(codec_error_from)?
                .to_be_bytes(),
        );
    let edit_position = mdia_type
        .checked_sub(4)
        .ok_or_else(|| codec_error("invalid M4A media atom position"))?;
    bytes.splice(edit_position..edit_position, edit);
    Ok(bytes)
}

fn edit_list_atom(duration_samples: usize) -> Result<Vec<u8>> {
    let duration = u32::try_from(duration_samples).map_err(codec_error_from)?;
    let mut elst = Vec::with_capacity(28);
    elst.extend_from_slice(&[0, 0, 0, 0]); // version and flags
    elst.extend_from_slice(&1u32.to_be_bytes());
    elst.extend_from_slice(&duration.to_be_bytes());
    elst.extend_from_slice(&0i32.to_be_bytes()); // no leading media-time trim
    elst.extend_from_slice(&1i16.to_be_bytes()); // playback rate 1.0
    elst.extend_from_slice(&0i16.to_be_bytes());
    let elst = atom(*b"elst", &elst)?;
    atom(*b"edts", &elst)
}

fn atom(kind: [u8; 4], payload: &[u8]) -> Result<Vec<u8>> {
    let size = u32::try_from(
        payload
            .len()
            .checked_add(8)
            .ok_or_else(|| codec_error("M4A atom size overflow"))?,
    )
    .map_err(codec_error_from)?;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(payload);
    Ok(out)
}

fn find_last(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| codec_error("M4A atom offset overflow"))?;
    let data = bytes
        .get(offset..end)
        .ok_or_else(|| codec_error("truncated M4A atom"))?;
    Ok(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}

fn split_adts(bytes: &[u8]) -> Result<Vec<&[u8]>> {
    let mut frames = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if bytes.len() - position < 7
            || bytes[position] != 0xff
            || bytes[position + 1] & 0xf6 != 0xf0
        {
            return Err(codec_error("AAC encoder returned invalid ADTS framing"));
        }
        let header = if bytes[position + 1] & 1 == 0 { 9 } else { 7 };
        let length = (usize::from(bytes[position + 3] & 3) << 11)
            | (usize::from(bytes[position + 4]) << 3)
            | usize::from(bytes[position + 5] >> 5);
        if length < header || position + length > bytes.len() {
            return Err(codec_error("AAC encoder returned a truncated ADTS frame"));
        }
        frames.push(&bytes[position + header..position + length]);
        position += length;
    }
    Ok(frames)
}

fn codec_error(message: impl Into<String>) -> Error {
    Error::AudioCodec(message.into())
}
fn codec_error_from(error: impl std::fmt::Display) -> Error {
    codec_error(error.to_string())
}

fn within_duration(samples: usize, sample_rate: u32) -> bool {
    within_duration_with_padding(samples, sample_rate, 0)
}

fn within_duration_with_padding(samples: usize, sample_rate: u32, padding_samples: usize) -> bool {
    let maximum = u64::from(sample_rate)
        .saturating_mul(MAX_DURATION_SECONDS)
        .saturating_add(u64::try_from(padding_samples).unwrap_or(u64::MAX));
    u64::try_from(samples).is_ok_and(|count| count <= maximum)
}

/// Convert clamped normalized PCM to the encoder's signed integer range.
#[allow(clippy::cast_possible_truncation)]
fn pcm16_sample(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
