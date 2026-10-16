//! Public API coverage for voice-note codecs using a checked-in WAV fixture.

use std::io::Cursor;
use tinyvoice::Result;
use tinyvoice::codec::{self, AudioCodec};

const WAV_FIXTURE: &[u8] = include_bytes!("fixtures/voice-pop.wav");

#[test]
fn probe_waveform_and_passthrough_work_for_a_real_wav_fixture() -> Result<()> {
    let info = codec::probe(WAV_FIXTURE)?;
    assert_eq!(info.codec, AudioCodec::WavPcm16);
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 1);
    assert!(info.duration_ms > 0);

    let peaks = codec::waveform(WAV_FIXTURE, 256)?;
    assert_eq!(peaks.len(), 256);
    assert!(peaks.iter().any(|peak| *peak > 0));
    assert_eq!(
        codec::transcode(WAV_FIXTURE, AudioCodec::WavPcm16)?,
        WAV_FIXTURE
    );
    Ok(())
}

#[test]
fn opus_and_aac_voice_notes_round_trip_through_independent_container_decoders() -> Result<()> {
    let opus = codec::transcode(WAV_FIXTURE, AudioCodec::OggOpus)?;
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

    let m4a = codec::transcode(&opus, AudioCodec::M4aAac)?;
    assert!(m4a.windows(4).any(|window| window == b"ftyp"));
    let aac_info = codec::probe(&m4a)?;
    assert_eq!(aac_info.codec, AudioCodec::M4aAac);
    assert_eq!(aac_info.sample_rate, 48_000);
    assert_eq!(aac_info.channels, 1);
    assert!(aac_info.duration_ms > 0);
    assert!(aac_info.duration_ms.abs_diff(opus_info.duration_ms) <= 1);
    assert!(codec::waveform(&m4a, 256)?.iter().any(|peak| *peak > 0));

    let wav = codec::transcode(&m4a, AudioCodec::WavPcm16)?;
    assert!(wav.starts_with(b"RIFF"));
    let wav_info = codec::probe(&wav)?;
    assert_eq!(wav_info.codec, AudioCodec::WavPcm16);
    assert_eq!(wav_info.sample_rate, 48_000);
    assert!(wav_info.duration_ms.abs_diff(opus_info.duration_ms) <= 1);
    Ok(())
}

#[test]
fn malformed_and_truncated_audio_are_rejected() {
    assert!(codec::probe(b"not audio").is_err());
    assert!(codec::probe(&WAV_FIXTURE[..WAV_FIXTURE.len() - 10]).is_err());
    assert!(codec::waveform(WAV_FIXTURE, 0).is_err());
    assert!(codec::waveform(WAV_FIXTURE, 4097).is_err());
}
