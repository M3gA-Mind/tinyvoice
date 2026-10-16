//! Public API coverage for voice-note codecs using a checked-in WAV fixture.

use tinyvoice::codec::{self, AudioCodec};
use tinyvoice::Result;

const WAV_FIXTURE: &[u8] = include_bytes!("voice-pop.wav");

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
    assert_eq!(codec::transcode(WAV_FIXTURE, AudioCodec::WavPcm16)?, WAV_FIXTURE);
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

    let m4a = codec::transcode(&opus, AudioCodec::M4aAac)?;
    let reader = mp4::Mp4Reader::read_header(std::io::Cursor::new(m4a.clone()), m4a.len() as u64).map_err(|error| { eprintln!("mp4 parse error: {error:?}"); tinyvoice::Error::AudioCodec(error.to_string()) })?;
    eprintln!("tracks {} duration {:?}", reader.tracks().len(), reader.duration());
    for track in reader.tracks().values() { eprintln!("track {:?}", track.track_type()); }
    assert!(m4a.windows(4).any(|window| window == b"ftyp"));
    let aac_info = codec::probe(&m4a)?;
    assert_eq!(aac_info.codec, AudioCodec::M4aAac);
    assert_eq!(aac_info.sample_rate, 48_000);
    assert_eq!(aac_info.channels, 1);
    assert!(aac_info.duration_ms > 0);

    let wav = codec::transcode(&m4a, AudioCodec::WavPcm16)?;
    assert!(wav.starts_with(b"RIFF"));
    let wav_info = codec::probe(&wav)?;
    assert_eq!(wav_info.codec, AudioCodec::WavPcm16);
    assert_eq!(wav_info.sample_rate, 48_000);
    Ok(())
}

#[test]
fn malformed_and_truncated_audio_are_rejected() {
    assert!(codec::probe(b"not audio").is_err());
    assert!(codec::probe(&WAV_FIXTURE[..WAV_FIXTURE.len() - 10]).is_err());
    assert!(codec::waveform(WAV_FIXTURE, 0).is_err());
    assert!(codec::waveform(WAV_FIXTURE, 4097).is_err());
}
