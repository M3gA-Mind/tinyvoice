//! Internal codec boundaries, limits, and container-repair behavior.

use super::*;

#[test]
fn m4a_repair_rejects_invalid_muxer_output_without_panicking() {
    assert!(repair_m4a(Vec::new(), 1).is_err());
    assert!(repair_m4a(b"not an MP4".to_vec(), 1).is_err());
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
fn transcode_rejects_a_mismatched_source_declaration() {
    assert!(transcode(b"not a wav", AudioCodec::WavPcm16, AudioCodec::OggOpus).is_err());
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
    repair_m4a(writer.into_writer().into_inner(), 9_600)
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
