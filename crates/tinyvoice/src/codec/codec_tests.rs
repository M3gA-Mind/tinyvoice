//! Internal codec boundaries, limits, and container-repair behavior.

use super::*;

#[test]
fn m4a_repair_rejects_invalid_muxer_output_without_panicking() {
    assert!(repair_m4a(Vec::new(), 1).is_err());
    assert!(repair_m4a(b"not an MP4".to_vec(), 1).is_err());
}

#[test]
fn m4a_sl_config_descriptor_has_its_required_predefined_byte() -> Result<()> {
    let (raw_m4a, duration_samples) = stereo_aac_muxed_fixture()?;
    let raw_esds_type = find_last(&raw_m4a, b"esds").ok_or_else(|| codec_error("missing esds"))?;
    let raw_esds_end = raw_esds_type - 4 + read_be_u32(&raw_m4a, raw_esds_type - 4)? as usize;
    let raw_descriptors = raw_m4a
        .get(raw_esds_type + 8..raw_esds_end)
        .ok_or_else(|| codec_error("truncated raw esds"))?;
    let raw_sl_config = raw_descriptors
        .windows(2)
        .position(|window| window == [0x06, 0x00])
        .ok_or_else(|| codec_error("missing raw SLConfig descriptor"))?;
    assert_eq!(
        &raw_descriptors[raw_sl_config..raw_sl_config + 3],
        &[0x06, 0x00, 0x00]
    );

    let m4a = repair_m4a(raw_m4a, duration_samples)?;
    let esds_type = find_last(&m4a, b"esds").ok_or_else(|| codec_error("missing esds"))?;
    let esds_start = esds_type - 4;
    let esds_end = esds_start + read_be_u32(&m4a, esds_start)? as usize;
    let descriptor_start = esds_type + 8;
    let descriptors = m4a
        .get(descriptor_start..esds_end)
        .ok_or_else(|| codec_error("truncated esds"))?;
    let sl_config = descriptors
        .windows(2)
        .position(|window| window == [0x06, 0x01])
        .ok_or_else(|| codec_error("missing SLConfig descriptor"))?;
    assert_eq!(&descriptors[sl_config..sl_config + 3], &[0x06, 0x01, 0x02]);
    assert!(
        probe(&m4a).is_ok(),
        "repaired M4A remains independently parseable"
    );
    Ok(())
}

#[test]
fn normalized_waveform_peak_uses_the_full_byte_range() {
    assert_eq!(normalized_peak_byte(0.0), 0);
    assert_eq!(normalized_peak_byte(0.5), 128);
    assert_eq!(normalized_peak_byte(1.0), 255);
    assert_eq!(normalized_peak_byte(2.0), 255);
}

#[test]
fn decoded_duration_limit_scales_with_input_sample_rate() {
    assert!(within_duration(8_000 * 300, 8_000));
    assert!(!within_duration(8_000 * 300 + 1, 8_000));
}

#[test]
fn aac_frame_padding_is_bounded_and_only_applies_to_the_decode_stage() {
    let five_minutes = 48_000usize * 300;
    assert!(within_duration_with_padding(
        five_minutes + AAC_FRAME_SAMPLES,
        48_000,
        AAC_FRAME_SAMPLES
    ));
    assert!(!within_duration_with_padding(
        five_minutes + AAC_FRAME_SAMPLES + 1,
        48_000,
        AAC_FRAME_SAMPLES
    ));
    assert!(!within_duration(five_minutes + 1, 48_000));
}

#[test]
fn five_minute_m4a_trims_real_aac_frame_padding_and_rejects_longer_duration() -> Result<()> {
    let exact = five_minute_silent_aac_fixture(48_000 * 300)?;
    assert_eq!(probe(&exact)?.duration_ms, 300_000);

    let over = five_minute_silent_aac_fixture(48_000 * 300 + 1)?;
    assert!(probe(&over).is_err());
    Ok(())
}

#[test]
fn m4a_duration_uses_the_selected_symphonia_track_id() -> Result<()> {
    let bytes = multi_track_aac_fixture()?;
    let mut hint = Hint::new();
    hint.with_extension("m4a");
    let source = MediaSourceStream::new(
        Box::new(Cursor::new(bytes.clone())),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(codec_error_from)?;
    let selected_track_id = format
        .default_track(symphonia::core::formats::TrackType::Audio)
        .ok_or_else(|| codec_error("test M4A has no default audio track"))?
        .id;
    let mp4 = mp4::Mp4Reader::read_header(Cursor::new(bytes.clone()), bytes.len() as u64)
        .map_err(codec_error_from)?;
    let selected = mp4
        .tracks()
        .get(&selected_track_id)
        .ok_or_else(|| codec_error("Symphonia ID does not match MP4 track ID"))?;
    let other = mp4
        .tracks()
        .values()
        .find(|track| track.track_id() != selected_track_id)
        .ok_or_else(|| codec_error("test M4A is missing its second audio track"))?;
    assert_ne!(selected.duration(), other.duration());

    let (_, samples, rate, _) = decode_symphonia(&bytes, AudioCodec::M4aAac, "m4a")?;
    let expected = m4a_playable_samples(&mp4, selected_track_id, rate, usize::MAX)?;
    assert_eq!(samples.len(), expected);
    let media_duration = selected.trak.mdia.mdhd.duration;
    let media_timescale = selected.trak.mdia.mdhd.timescale;
    assert_eq!(
        u128::try_from(expected).map_err(codec_error_from)?,
        u128::from(media_duration) * u128::from(rate) / u128::from(media_timescale)
    );
    Ok(())
}

#[test]
fn invalid_wav_sample_rates_and_channel_counts_are_rejected() -> Result<()> {
    let mut low_rate = audio::f32_mono_to_wav(&[0.2; 960], VOICE_RATE)?;
    low_rate[24..28].copy_from_slice(&1_000u32.to_le_bytes());
    low_rate[28..32].copy_from_slice(&2_000u32.to_le_bytes());
    assert!(probe(&low_rate).is_err());

    let mut many_channels = audio::f32_mono_to_wav(&[0.2; 9], VOICE_RATE)?;
    many_channels[22..24].copy_from_slice(&9u16.to_le_bytes());
    many_channels[28..32].copy_from_slice(&(48_000u32 * 9 * 2).to_le_bytes());
    many_channels[32..34].copy_from_slice(&(9u16 * 2).to_le_bytes());
    assert!(probe(&many_channels).is_err());
    Ok(())
}

#[test]
fn symphonia_rejects_the_dedicated_ogg_codec() -> Result<()> {
    let wav = audio::f32_mono_to_wav(&[0.2; 960], VOICE_RATE)?;
    assert!(decode_symphonia(&wav, AudioCodec::OggOpus, "wav").is_err());
    assert!(decode_symphonia(&wav, AudioCodec::M4aAac, "wav").is_err());
    Ok(())
}

#[test]
fn transcode_rejects_a_mismatched_source_declaration() {
    assert!(transcode(b"not a wav", AudioCodec::WavPcm16, AudioCodec::OggOpus).is_err());
}

#[test]
fn empty_silent_oversized_and_unsupported_codec_inputs_are_rejected() -> Result<()> {
    assert!(probe(b"").is_err());
    assert!(waveform(b"", 1).is_err());
    assert!(transcode(b"", AudioCodec::WavPcm16, AudioCodec::OggOpus).is_err());
    let oversized = vec![0; MAX_INPUT_BYTES + 1];
    assert!(probe(&oversized).is_err());
    assert!(transcode(&oversized, AudioCodec::WavPcm16, AudioCodec::OggOpus).is_err());

    let empty_wav = audio::f32_mono_to_wav(&[], VOICE_RATE)?;
    assert!(waveform(&empty_wav, 1).is_err());

    let silence = audio::f32_mono_to_wav(&vec![0.0; 960], VOICE_RATE)?;
    assert_eq!(waveform(&silence, 32)?, vec![0; 32]);
    assert!(transcode(&silence, AudioCodec::WavPcm16, AudioCodec::Mp3).is_err());
    assert!(encode_opus(&[]).is_err());
    assert!(encode_aac_m4a(&[]).is_err());
    Ok(())
}

#[test]
fn adts_splitter_rejects_bad_and_truncated_frames() {
    assert!(split_adts(b"not ADTS").is_err());
    assert!(split_adts(&[0xff, 0xf1, 0, 0, 0, 0]).is_err());
    assert!(split_adts(&[0xff, 0xf1, 0, 0, 0, 0, 0]).is_err());
}

#[test]
fn waveform_bucket_indices_use_wide_math_for_32_bit_audio_limits() -> Result<()> {
    let clip_samples = 23 * 48_000;
    let buckets = 4096;
    let (start, end) = waveform_bucket_range(clip_samples, buckets, buckets - 1)?;
    assert_eq!(start, (buckets - 1) * clip_samples / buckets);
    assert_eq!(end, clip_samples);

    let sample_count = usize::try_from(u32::MAX).map_err(codec_error_from)?;
    let index = buckets - 1;
    assert!(
        u32::try_from(sample_count)
            .map_err(codec_error_from)?
            .checked_mul(u32::try_from(index).map_err(codec_error_from)?)
            .is_none()
    );

    let expected_start = usize::try_from(
        (u128::try_from(index).map_err(codec_error_from)?
            * u128::try_from(sample_count).map_err(codec_error_from)?)
            / u128::try_from(buckets).map_err(codec_error_from)?,
    )
    .map_err(codec_error_from)?;
    let expected_end = usize::try_from(
        (u128::try_from(index + 1).map_err(codec_error_from)?
            * u128::try_from(sample_count).map_err(codec_error_from)?)
            / u128::try_from(buckets).map_err(codec_error_from)?,
    )
    .map_err(codec_error_from)?;
    assert_eq!(
        waveform_bucket_range(sample_count, buckets, index)?,
        (expected_start, expected_end)
    );
    Ok(())
}

#[test]
fn an_empty_ogg_opus_stream_has_no_playable_audio() -> Result<()> {
    let head = OpusHead {
        channel_count: 1,
        pre_skip: 0,
        input_sample_rate: VOICE_RATE,
        output_gain_q8: 0,
        mapping_family: 0,
        stream_count: 1,
        coupled_count: 0,
        channel_mapping: vec![0],
    };
    let bytes = OggOpusWriter::new(Vec::new(), head)
        .map_err(codec_error_from)?
        .finish()
        .map_err(codec_error_from)?;
    assert!(decode_opus(&bytes).is_err());
    Ok(())
}

#[test]
fn same_format_aac_transcode_normalizes_stereo_to_mono_48khz() -> Result<()> {
    let stereo = stereo_aac_fixture()?;
    let input = probe(&stereo)?;
    assert_eq!(input.codec, AudioCodec::M4aAac);
    assert_eq!(input.channels, 2);

    let mono = transcode(&stereo, AudioCodec::M4aAac, AudioCodec::M4aAac)?;
    let output = probe(&mono)?;
    assert_eq!(output.channels, 1);
    assert_eq!(output.sample_rate, 48_000);
    Ok(())
}

fn stereo_aac_fixture() -> Result<Vec<u8>> {
    let (bytes, duration_samples) = stereo_aac_muxed_fixture()?;
    repair_m4a(bytes, duration_samples)
}

fn stereo_aac_muxed_fixture() -> Result<(Vec<u8>, usize)> {
    use bytes::Bytes;
    use mp4::{
        AacConfig, AudioObjectType, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample,
        Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
    };
    use oxideav_aac::encoder::{EncoderConfig, StreamEncoder};

    let pcm: Vec<i16> = (0..9_600).flat_map(|_| [6_000, 6_000]).collect();
    let mut encoder = StreamEncoder::new(EncoderConfig {
        sample_rate: 48_000,
        channels: 2,
        bitrate: 128_000,
    })
    .map_err(codec_error_from)?;
    let adts = encoder.encode_all(&pcm).map_err(codec_error_from)?;
    let frames = aac_frames(&adts)?;
    let track = TrackConfig {
        track_type: TrackType::Audio,
        timescale: VOICE_RATE,
        language: "und".into(),
        media_conf: MediaConfig::AacConfig(AacConfig {
            bitrate: 128_000,
            profile: AudioObjectType::AacLowComplexity,
            freq_index: SampleFreqIndex::Freq48000,
            chan_conf: ChannelConfig::Stereo,
        }),
    };
    let config = Mp4Config {
        major_brand: FourCC::from(*b"M4A "),
        minor_version: 0,
        compatible_brands: vec![FourCC::from(*b"M4A "), FourCC::from(*b"isom")],
        timescale: VOICE_RATE,
    };
    let mut writer =
        Mp4Writer::write_start(Cursor::new(Vec::new()), &config).map_err(codec_error_from)?;
    writer.add_track(&track).map_err(codec_error_from)?;
    let mut start_time = 0u64;
    for frame in frames.iter().skip(1) {
        writer
            .write_sample(
                1,
                &Mp4Sample {
                    start_time,
                    duration: 1024,
                    rendering_offset: 0,
                    is_sync: true,
                    bytes: Bytes::copy_from_slice(frame),
                },
            )
            .map_err(codec_error_from)?;
        start_time += 1024;
    }
    writer.write_end().map_err(codec_error_from)?;
    Ok((writer.into_writer().into_inner(), 9_600))
}

fn five_minute_silent_aac_fixture(duration_samples: usize) -> Result<Vec<u8>> {
    use bytes::Bytes;
    use mp4::{
        AacConfig, AudioObjectType, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample,
        Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
    };
    use oxideav_aac::encoder::{EncoderConfig, StreamEncoder};

    let mut encoder = StreamEncoder::new(EncoderConfig {
        sample_rate: 48_000,
        channels: 1,
        bitrate: 64_000,
    })
    .map_err(codec_error_from)?;
    let adts = encoder
        .encode_all(&vec![0; AAC_FRAME_SAMPLES * 2])
        .map_err(codec_error_from)?;
    let frame = *aac_frames(&adts)?
        .get(1)
        .ok_or_else(|| codec_error("test AAC encoder returned too few frames"))?;
    let track = TrackConfig {
        track_type: TrackType::Audio,
        timescale: VOICE_RATE,
        language: "und".into(),
        media_conf: MediaConfig::AacConfig(AacConfig {
            bitrate: 64_000,
            profile: AudioObjectType::AacLowComplexity,
            freq_index: SampleFreqIndex::Freq48000,
            chan_conf: ChannelConfig::Mono,
        }),
    };
    let config = Mp4Config {
        major_brand: FourCC::from(*b"M4A "),
        minor_version: 0,
        compatible_brands: vec![FourCC::from(*b"M4A "), FourCC::from(*b"isom")],
        timescale: VOICE_RATE,
    };
    let mut writer =
        Mp4Writer::write_start(Cursor::new(Vec::new()), &config).map_err(codec_error_from)?;
    writer.add_track(&track).map_err(codec_error_from)?;
    let full_frames = duration_samples / AAC_FRAME_SAMPLES;
    let final_duration = duration_samples % AAC_FRAME_SAMPLES;
    let sample_count = full_frames + usize::from(final_duration > 0);
    for index in 0..sample_count {
        let start_time = u64::try_from(index * AAC_FRAME_SAMPLES).map_err(codec_error_from)?;
        let duration = if index == full_frames && final_duration > 0 {
            final_duration
        } else {
            AAC_FRAME_SAMPLES
        };
        writer
            .write_sample(
                1,
                &Mp4Sample {
                    start_time,
                    duration: u32::try_from(duration).map_err(codec_error_from)?,
                    rendering_offset: 0,
                    is_sync: true,
                    bytes: Bytes::copy_from_slice(frame),
                },
            )
            .map_err(codec_error_from)?;
    }
    writer.write_end().map_err(codec_error_from)?;
    repair_m4a(writer.into_writer().into_inner(), duration_samples)
}

fn multi_track_aac_fixture() -> Result<Vec<u8>> {
    use bytes::Bytes;
    use mp4::{
        AacConfig, AudioObjectType, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample,
        Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
    };
    use oxideav_aac::encoder::{EncoderConfig, StreamEncoder};

    let mut encoder = StreamEncoder::new(EncoderConfig {
        sample_rate: 48_000,
        channels: 2,
        bitrate: 128_000,
    })
    .map_err(codec_error_from)?;
    let adts = encoder
        .encode_all(&vec![4_000; AAC_FRAME_SAMPLES * 12 * 2])
        .map_err(codec_error_from)?;
    let frames = aac_frames(&adts)?;
    let track = TrackConfig {
        track_type: TrackType::Audio,
        timescale: VOICE_RATE,
        language: "und".into(),
        media_conf: MediaConfig::AacConfig(AacConfig {
            bitrate: 128_000,
            profile: AudioObjectType::AacLowComplexity,
            freq_index: SampleFreqIndex::Freq48000,
            chan_conf: ChannelConfig::Stereo,
        }),
    };
    let config = Mp4Config {
        major_brand: FourCC::from(*b"M4A "),
        minor_version: 0,
        compatible_brands: vec![FourCC::from(*b"M4A "), FourCC::from(*b"isom")],
        timescale: VOICE_RATE,
    };
    let mut writer =
        Mp4Writer::write_start(Cursor::new(Vec::new()), &config).map_err(codec_error_from)?;
    writer.add_track(&track).map_err(codec_error_from)?;
    writer.add_track(&track).map_err(codec_error_from)?;
    for (track_id, frame_count) in [(1, 4), (2, 9)] {
        let mut start_time = 0u64;
        for frame in frames.iter().skip(1).take(frame_count) {
            writer
                .write_sample(
                    track_id,
                    &Mp4Sample {
                        start_time,
                        duration: AAC_FRAME_SAMPLES as u32,
                        rendering_offset: 0,
                        is_sync: true,
                        bytes: Bytes::copy_from_slice(frame),
                    },
                )
                .map_err(codec_error_from)?;
            start_time += AAC_FRAME_SAMPLES as u64;
        }
    }
    writer.write_end().map_err(codec_error_from)?;
    Ok(writer.into_writer().into_inner())
}

fn aac_frames(adts: &[u8]) -> Result<Vec<&[u8]>> {
    let mut frames = Vec::new();
    let mut position = 0;
    while position < adts.len() {
        if adts.len() - position < 7 || adts[position] != 0xff {
            return Err(codec_error("invalid test AAC framing"));
        }
        let header = if adts[position + 1] & 1 == 0 { 9 } else { 7 };
        let length = (usize::from(adts[position + 3] & 3) << 11)
            | (usize::from(adts[position + 4]) << 3)
            | usize::from(adts[position + 5] >> 5);
        if length < header || position + length > adts.len() {
            return Err(codec_error("truncated test AAC frame"));
        }
        frames.push(&adts[position + header..position + length]);
        position += length;
    }
    Ok(frames)
}
