//! Public API coverage for voice-note codecs using a checked-in WAV fixture.

use std::io::Cursor;
use tinyvoice::Result;
use tinyvoice::codec::{self, AudioCodec};

const VOICE_ENVELOPE_FIXTURE: &[u8] = include_bytes!("fixtures/voice-envelope.wav");
const OPUS_INTEROP_FIXTURE: &[u8] = include_bytes!("fixtures/opus-interop.wav");
const MP3_FIXTURE: &[u8] = include_bytes!("fixtures/mp3-test.bit");

fn window_rms(bytes: &[u8], sample_rate: u32, window_ms: usize) -> Vec<f64> {
    let samples = bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f64 / i16::MAX as f64)
        .collect::<Vec<_>>();
    let window = sample_rate as usize * window_ms / 1_000;
    samples
        .chunks(window)
        .map(|window| {
            (window.iter().map(|sample| sample * sample).sum::<f64>() / window.len() as f64).sqrt()
        })
        .collect()
}

fn assert_envelope_retained(reference: &[u8], reference_rate: u32, decoded: &[u8]) {
    let expected = window_rms(reference, reference_rate, 40);
    let actual = window_rms(decoded, 48_000, 40);
    assert_eq!(
        actual.len(),
        expected.len(),
        "decoded clip duration changed"
    );
    let scale = expected.iter().sum::<f64>() / actual.iter().sum::<f64>();
    let normalized_error = expected
        .iter()
        .zip(actual.iter())
        .map(|(expected, actual)| (expected - actual * scale).abs())
        .sum::<f64>()
        / expected.iter().sum::<f64>();
    assert!(
        normalized_error < 0.22,
        "voice envelope drifted: {normalized_error:.3}"
    );
    assert!(
        actual.iter().any(|rms| *rms > 0.05),
        "decoded signal is silent"
    );
}

#[test]
fn probe_waveform_and_passthrough_work_for_a_real_wav_fixture() -> Result<()> {
    let info = codec::probe(VOICE_ENVELOPE_FIXTURE)?;
    assert_eq!(info.codec, AudioCodec::WavPcm16);
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 1);
    assert!(info.duration_ms > 2_300);

    let peaks = codec::waveform(VOICE_ENVELOPE_FIXTURE, 256)?;
    assert_eq!(peaks.len(), 256);
    assert!(peaks.iter().any(|peak| *peak > 0));
    assert_eq!(
        codec::transcode(
            VOICE_ENVELOPE_FIXTURE,
            AudioCodec::WavPcm16,
            AudioCodec::WavPcm16
        )?,
        VOICE_ENVELOPE_FIXTURE
    );
    Ok(())
}

#[test]
fn opus_and_aac_voice_notes_round_trip_through_independent_container_decoders() -> Result<()> {
    let opus = codec::transcode(
        OPUS_INTEROP_FIXTURE,
        AudioCodec::WavPcm16,
        AudioCodec::OggOpus,
    )?;
    assert!(opus.starts_with(b"OggS"));
    let opus_info = codec::probe(&opus)?;
    assert_eq!(opus_info.codec, AudioCodec::OggOpus);
    assert_eq!(opus_info.sample_rate, 48_000);
    assert_eq!(opus_info.channels, 1);
    assert!(opus_info.duration_ms > 0);

    // Decode the emitted packets with a separately implemented Opus decoder;
    // this guards against an encoder/decoder pair agreeing on invalid packets.
    let mut packet_reader = opus_pure::OggOpusReader::new(Cursor::new(opus.clone()))
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
    let mut independent = opus_decoder::OpusDecoder::new(48_000, 1)
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
    let mut independent_samples = Vec::new();
    while let Some(packet) = packet_reader
        .read_packet()
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?
    {
        let mut frame = vec![0.0; opus_decoder::OpusDecoder::MAX_FRAME_SIZE_48K];
        let count = independent
            .decode_float(&packet.data, &mut frame, false)
            .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
        independent_samples.extend_from_slice(&frame[..count]);
    }
    assert!(
        independent_samples
            .iter()
            .any(|sample| sample.abs() > 0.001)
    );

    let m4a = codec::transcode(&opus, AudioCodec::OggOpus, AudioCodec::M4aAac)?;
    assert!(m4a.windows(4).any(|window| window == b"ftyp"));
    let aac_info = codec::probe(&m4a)?;
    assert_eq!(aac_info.codec, AudioCodec::M4aAac);
    assert_eq!(aac_info.sample_rate, 48_000);
    assert_eq!(aac_info.channels, 1);
    assert!(aac_info.duration_ms > 0);
    assert!(aac_info.duration_ms.abs_diff(opus_info.duration_ms) <= 1);
    assert!(codec::waveform(&m4a, 256)?.iter().any(|peak| *peak > 0));

    let wav = codec::transcode(&m4a, AudioCodec::M4aAac, AudioCodec::WavPcm16)?;
    assert!(wav.starts_with(b"RIFF"));
    let wav_info = codec::probe(&wav)?;
    assert_eq!(wav_info.codec, AudioCodec::WavPcm16);
    assert_eq!(wav_info.sample_rate, 48_000);
    assert!(wav_info.duration_ms.abs_diff(opus_info.duration_ms) <= 1);
    Ok(())
}

#[test]
fn multiframe_voice_survives_opus_aac_round_trip_with_gapless_duration() -> Result<()> {
    let original = codec::transcode(
        VOICE_ENVELOPE_FIXTURE,
        AudioCodec::WavPcm16,
        AudioCodec::WavPcm16,
    )?;
    let original_info = codec::probe(&original)?;
    assert!(original_info.duration_ms > 2_300);

    let opus = codec::transcode(&original, AudioCodec::WavPcm16, AudioCodec::OggOpus)?;
    let aac = codec::transcode(&opus, AudioCodec::OggOpus, AudioCodec::M4aAac)?;
    let wav = codec::transcode(&aac, AudioCodec::M4aAac, AudioCodec::WavPcm16)?;
    let decoded_info = codec::probe(&wav)?;
    assert_eq!(decoded_info.sample_rate, 48_000);
    assert!(decoded_info.duration_ms.abs_diff(original_info.duration_ms) <= 1);
    assert_envelope_retained(&original[44..], original_info.sample_rate, &wav[44..]);
    Ok(())
}

#[test]
fn mp3_tts_source_decodes_and_transcodes_to_voice_formats() -> Result<()> {
    let info = codec::probe(MP3_FIXTURE)?;
    assert_eq!(info.codec, AudioCodec::Mp3);
    assert!(info.duration_ms > 0);

    let opus = codec::transcode(MP3_FIXTURE, AudioCodec::Mp3, AudioCodec::OggOpus)?;
    let opus_info = codec::probe(&opus)?;
    assert_eq!(opus_info.channels, 1);
    assert_eq!(opus_info.sample_rate, 48_000);

    let wav = codec::transcode(MP3_FIXTURE, AudioCodec::Mp3, AudioCodec::WavPcm16)?;
    let wav_info = codec::probe(&wav)?;
    assert_eq!(wav_info.channels, 1);
    assert_eq!(wav_info.sample_rate, 48_000);
    assert!(wav_info.duration_ms.abs_diff(info.duration_ms) <= 30);
    let aac = codec::transcode(MP3_FIXTURE, AudioCodec::Mp3, AudioCodec::M4aAac)?;
    let aac_info = codec::probe(&aac)?;
    assert_eq!(aac_info.codec, AudioCodec::M4aAac);
    assert_eq!(aac_info.sample_rate, 48_000);
    assert!(aac_info.duration_ms.abs_diff(wav_info.duration_ms) <= 1);
    let aac_wav = codec::transcode(&aac, AudioCodec::M4aAac, AudioCodec::WavPcm16)?;
    assert_envelope_retained(&wav[44..], wav_info.sample_rate, &aac_wav[44..]);
    assert!(codec::transcode(MP3_FIXTURE, AudioCodec::WavPcm16, AudioCodec::OggOpus).is_err());
    Ok(())
}

#[test]
fn same_format_opus_transcode_normalizes_stereo_to_mono_48khz() -> Result<()> {
    let mut encoder = opus_pure::OpusEncoder::new(48_000, 2, opus_pure::Application::Voip)
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
    let head = opus_pure::OpusHead::for_encoder(&encoder, 48_000);
    let mut writer = opus_pure::OggOpusWriter::new(Vec::new(), head.clone())
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
    let pcm: Vec<f32> = (0..960).flat_map(|_| [0.1, 0.1]).collect();
    let mut packet = vec![0; opus_pure::MAX_PACKET_BYTES];
    let count = encoder
        .encode(&pcm, 960, &mut packet)
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
    writer
        .write_packet_with_duration(&packet[..count], 960)
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;
    let stereo = writer
        .finish()
        .map_err(|error| tinyvoice::Error::AudioCodec(error.to_string()))?;

    let mono = codec::transcode(&stereo, AudioCodec::OggOpus, AudioCodec::OggOpus)?;
    let info = codec::probe(&mono)?;
    assert_eq!(info.codec, AudioCodec::OggOpus);
    assert_eq!(info.channels, 1);
    assert_eq!(info.sample_rate, 48_000);
    Ok(())
}

#[test]
fn malformed_and_truncated_audio_are_rejected() {
    assert!(codec::probe(b"not audio").is_err());
    assert!(codec::probe(&VOICE_ENVELOPE_FIXTURE[..VOICE_ENVELOPE_FIXTURE.len() - 10]).is_err());
    assert!(codec::waveform(VOICE_ENVELOPE_FIXTURE, 0).is_err());
    assert!(codec::waveform(VOICE_ENVELOPE_FIXTURE, 4097).is_err());
    let truncated_opus = codec::transcode(
        VOICE_ENVELOPE_FIXTURE,
        AudioCodec::WavPcm16,
        AudioCodec::OggOpus,
    )
    .expect("valid fixture encodes");
    assert!(codec::probe(&truncated_opus[..truncated_opus.len() / 2]).is_err());
    assert!(
        codec::transcode(
            &truncated_opus[..truncated_opus.len() / 2],
            AudioCodec::OggOpus,
            AudioCodec::WavPcm16,
        )
        .is_err()
    );
    let truncated_aac = codec::transcode(
        VOICE_ENVELOPE_FIXTURE,
        AudioCodec::WavPcm16,
        AudioCodec::M4aAac,
    )
    .expect("valid fixture encodes");
    assert!(codec::probe(&truncated_aac[..truncated_aac.len() / 2]).is_err());
    assert!(
        codec::transcode(
            &truncated_aac[..truncated_aac.len() / 2],
            AudioCodec::M4aAac,
            AudioCodec::WavPcm16,
        )
        .is_err()
    );
}
