//! Integration tests for the bus adapter, over a real in-memory `TinyBus`.
//!
//! These assert the *wire* contract — method names, payload shapes, error
//! wording — rather than the behaviour underneath, which the `tinyvoice` unit
//! tests already cover. A change that only moves logic should leave every
//! assertion here untouched; one that fires means the contract moved.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use super::{MAX_AUDIO_BYTES, MAX_SESSIONS, VoiceService, setup};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use tinybus::broker::Broker;
use tinybus::transport::memory::MemoryBus;
use tinybus::{Connection, Interface};
use tinyvoice_bus::{
    ActivationMode, AudioCodec, CodecInfo, CodecInput, CodecOutput, CodecTranscodeRequest,
    HostKeyFact, HotkeyFeedRequest, HotkeyHandleRequest, HotkeyReadRequest, HotkeyRequest,
    HotkeyReserveRequest, HotkeySource, SequencedHostFact, WaveformRequest, WaveformResult,
};
use tinyvoice_bus::{BUS_NAME, OBJECT_PATH, names};

/// A live bus with the interface served on it.
///
/// The service `Connection` is held here rather than dropped at the end of
/// setup: a peer that goes out of scope releases its bus name, and every call
/// then fails with `NameHasNoOwner` — which looks like a registration bug
/// rather than a lifetime one.
struct Served {
    proxy: tinybus::Proxy,
    _service: Connection,
}

impl std::ops::Deref for Served {
    type Target = tinybus::Proxy;

    fn deref(&self) -> &Self::Target {
        &self.proxy
    }
}

/// Stand up a broker, serve the interface, and hand back a client proxy.
async fn connect() -> tinybus::Result<Served> {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());

    let service = Connection::connect(bus.connect().await?).await?;
    setup(service.clone()).await?;

    let client = Connection::connect(bus.connect().await?).await?;
    Ok(Served {
        proxy: client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)?,
        _service: service,
    })
}

/// Little-endian `f32` samples, base64'd, as the wire carries them.
fn encode_samples(samples: &[f32]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    BASE64.encode(bytes)
}

#[test]
fn declared_methods_match_the_dispatch_table() {
    // The `module_export!` manifest lists these names separately from the
    // `#[interface]` impl, so nothing but this test keeps the two in step. A
    // manifest that over-claims makes the host advertise a method that is not
    // there; one that under-claims hides a method that is.
    let mut methods = VoiceService::default()
        .members()
        .into_iter()
        .map(|member| member.to_string())
        .collect::<Vec<_>>();
    methods.sort_unstable();

    assert_eq!(
        methods,
        names::METHODS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn route_returns_a_tagged_intent() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let json: String = proxy.call("Route", ("please pause the music",)).await?;
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["intent"], "pause");
    Ok(())
}

#[tokio::test]
async fn voice_note_codec_methods_are_available_over_the_module_contract() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let wav = include_bytes!("../../../tinyvoice/tests/fixtures/voice-pop.wav");
    let encoded = BASE64.encode(wav);

    let info: CodecInfo = proxy
        .call(
            "CodecProbe",
            (CodecInput {
                audio_base64: encoded.clone(),
            },),
        )
        .await?;
    assert_eq!(info.codec, AudioCodec::WavPcm16);
    assert_eq!(info.channels, 1);

    let waveform: WaveformResult = proxy
        .call(
            "CodecWaveform",
            (WaveformRequest {
                audio_base64: encoded.clone(),
                buckets: 256,
            },),
        )
        .await?;
    assert_eq!(waveform.peaks.len(), 256);
    assert!(waveform.peaks.iter().any(|peak| *peak > 0));

    let opus: CodecOutput = proxy
        .call(
            "CodecTranscode",
            (CodecTranscodeRequest {
                audio_base64: encoded,
                from: AudioCodec::WavPcm16,
                target: AudioCodec::OggOpus,
            },),
        )
        .await?;
    let opus = base64::engine::general_purpose::STANDARD
        .decode(opus.audio_base64)
        .expect("module returns valid base64");
    assert!(opus.starts_with(b"OggS"));
    Ok(())
}

#[tokio::test]
async fn route_carries_the_payload_for_a_parameterised_intent() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let json: String = proxy.call("Route", ("set volume to 40",)).await?;
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["intent"], "set_volume");
    assert_eq!(value["percent"], 40);
    Ok(())
}

#[tokio::test]
async fn an_unrecognised_transcript_comes_back_as_unknown_not_an_error() -> tinybus::Result<()> {
    // Routing may only ever shortcut. "I could not classify this" is a normal
    // answer the host acts on by calling its agent, not a fault.
    let proxy = connect().await?;
    let json: String = proxy
        .call("Route", ("what is the weather in Tokyo",))
        .await?;
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["intent"], "unknown");
    Ok(())
}

#[tokio::test]
async fn extract_command_strips_the_wake_word() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let command: String = proxy
        .call("ExtractCommand", ("hey tiny open slack", "Hey Tiny"))
        .await?;

    assert_eq!(command, "open slack");
    Ok(())
}

#[tokio::test]
async fn an_unaddressed_utterance_yields_an_empty_command() -> tinybus::Result<()> {
    let proxy = connect().await?;

    // Not addressed at all, and addressed with nothing after it, are the same
    // outcome for a caller: there is no command to route.
    let unaddressed: String = proxy
        .call("ExtractCommand", ("open slack", "Hey Tiny"))
        .await?;
    assert_eq!(unaddressed, "");

    let bare: String = proxy
        .call("ExtractCommand", ("hey tiny", "Hey Tiny"))
        .await?;
    assert_eq!(bare, "");
    Ok(())
}

#[tokio::test]
async fn wake_word_presence_distinguishes_a_bare_address() -> tinybus::Result<()> {
    let proxy = connect().await?;

    // This is what lets a host acknowledge "Hey Tiny" instead of staying
    // silent, which reads to the user as a dead microphone.
    let present: bool = proxy
        .call("WakeWordPresent", ("hey tiny", "Hey Tiny"))
        .await?;
    assert!(present);

    let absent: bool = proxy
        .call("WakeWordPresent", ("open slack", "Hey Tiny"))
        .await?;
    assert!(!absent);
    Ok(())
}

#[tokio::test]
async fn hallucination_modes_disagree_over_the_bus() -> tinybus::Result<()> {
    let proxy = connect().await?;

    let dictation: bool = proxy.call("IsHallucinated", ("okay", "dictation")).await?;
    let conversation: bool = proxy
        .call("IsHallucinated", ("okay", "conversation"))
        .await?;

    assert!(dictation, "a lone filler word is an artefact in dictation");
    assert!(!conversation, "the same word is a real reply in chat");
    Ok(())
}

#[tokio::test]
async fn an_unknown_mode_is_refused_rather_than_defaulted() -> tinybus::Result<()> {
    // The two modes disagree about real speech, so guessing would either
    // delete legitimate replies or leak artefacts into dictated text.
    let proxy = connect().await?;
    let result = proxy
        .call::<bool>("IsHallucinated", ("okay", "aggressive"))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "unknown mode unexpectedly succeeded",
        ));
    };
    assert!(
        error.to_string().contains("unknown hallucination mode"),
        "got: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn segment_reports_the_frame_each_event_landed_on() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let config = serde_json::json!({
        "onset_threshold": 0.1,
        "hangover_ms": 100,
        "min_speech_ms": 60,
        "max_utterance_ms": 5000,
    })
    .to_string();

    // Six loud frames (120ms voiced), then silence past the 100ms hangover.
    let mut energies = vec![0.5f32; 6];
    energies.extend(std::iter::repeat_n(0.0f32, 6));

    let json: String = proxy.call("Segment", (config, 20u32, energies)).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(events.len(), 2, "one start and one end: {events:?}");
    assert_eq!(events[0]["kind"], "speech_start");
    assert_eq!(events[0]["frame"], 0);
    assert_eq!(events[1]["kind"], "speech_end");
    assert_eq!(events[1]["emit"], true);
    assert_eq!(events[1]["forced"], false);
    // The index is what lets a host cut the audio in the right place.
    assert_eq!(events[1]["frame"], 10);
    Ok(())
}

#[tokio::test]
async fn a_malformed_vad_config_is_reported_not_defaulted() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<String>(
            "Segment",
            (r#"{"onset_threshold":0.1}"#, 20u32, vec![0.5f32]),
        )
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "partial config unexpectedly succeeded",
        ));
    };
    assert!(
        error.to_string().contains("invalid VAD config"),
        "got: {error}"
    );
    Ok(())
}

#[tokio::test]
async fn a_zero_frame_duration_is_refused() -> tinybus::Result<()> {
    // Every duration the segmenter reports is a multiple of this, so zero
    // would silently make every utterance 0ms long and fail `min_speech_ms`.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let result = proxy
        .call::<String>("Segment", (config, 0u32, vec![0.5f32]))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "zero frame_ms unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("frame_ms"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn encode_wav_round_trips_through_base64() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let samples = encode_samples(&[0.0, 0.5, -0.5]);

    let encoded: String = proxy.call("EncodeWav", (samples, 16_000u32)).await?;
    let wav = BASE64.decode(&encoded).expect("valid base64");

    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(wav.len(), 44 + 3 * 2);
    Ok(())
}

#[tokio::test]
async fn prepare_capture_downmixes_resamples_and_frames_in_one_call() -> tinybus::Result<()> {
    // Three round trips would ship the same audio across the bus three times
    // to do work that is microseconds of arithmetic.
    let proxy = connect().await?;
    // 400 interleaved samples = 200 stereo frames at 32 kHz
    //   -> 200 mono samples -> 100 samples at 16 kHz.
    let stereo: Vec<f32> = (0..400).map(|i| ((i as f32) / 20.0).sin() * 0.5).collect();

    let encoded: String = proxy
        .call(
            "PrepareCapture",
            (encode_samples(&stereo), 32_000u32, 2u16, 0.0f32),
        )
        .await?;
    let wav = BASE64.decode(&encoded).expect("valid base64");

    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(wav.len(), 44 + 100 * 2);
    // The header must declare the rate the samples were converted to, not the
    // one they arrived at.
    assert_eq!(
        u32::from_le_bytes(wav[24..28].try_into().expect("4 bytes")),
        16_000
    );
    Ok(())
}

#[tokio::test]
async fn a_ragged_channel_count_is_reported() -> tinybus::Result<()> {
    let proxy = connect().await?;
    // Three samples cannot be whole stereo frames.
    let result = proxy
        .call::<String>(
            "PrepareCapture",
            (encode_samples(&[0.1, 0.2, 0.3]), 32_000u32, 2u16, 0.0f32),
        )
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "ragged input unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("whole number"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn a_truncated_sample_buffer_is_refused() -> tinybus::Result<()> {
    let proxy = connect().await?;
    // Five bytes is not a whole number of f32 samples.
    let result = proxy
        .call::<String>("EncodeWav", (BASE64.encode([0u8; 5]), 16_000u32))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "truncated buffer unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("f32 samples"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn an_oversized_payload_is_refused_before_it_is_decoded() -> tinybus::Result<()> {
    // The length check runs on the encoded string. Allocating first and
    // validating after is how a hostile size becomes an allocation failure,
    // which aborts the host rather than returning an error it can handle.
    let proxy = connect().await?;
    let oversized = "A".repeat(MAX_AUDIO_BYTES / 3 * 4 + 8);
    let result = proxy
        .call::<String>("EncodeWav", (oversized, 16_000u32))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "oversized payload unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("exceeds"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn a_zero_sample_rate_is_refused() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<String>("EncodeWav", (encode_samples(&[0.1]), 0u32))
        .await;

    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "zero sample rate unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("sample rate"), "got: {error}");
    Ok(())
}

// --- The VAD session lifecycle ---
//
// This is the only state the module holds, so it is where a leak, a stale
// handle, or a lost utterance would come from.

#[tokio::test]
async fn a_session_segments_across_separate_calls() -> tinybus::Result<()> {
    // The whole point of a session: an utterance that spans two pushes must
    // survive the boundary. A stateless batch would lose the open segment.
    let proxy = connect().await?;
    let config = serde_json::json!({
        "onset_threshold": 0.1,
        "hangover_ms": 100,
        "min_speech_ms": 60,
        "max_utterance_ms": 5000,
    })
    .to_string();

    let session: u64 = proxy.call("VadOpen", (config,)).await?;

    // First push: speech starts and stays open.
    let json: String = proxy
        .call("VadPush", (session, 20u32, vec![0.5f32; 6]))
        .await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "speech_start");
    assert!(proxy.call::<bool>("VadIsSpeaking", (session,)).await?);

    // Second push: silence closes it, and the voiced total spans both calls.
    let json: String = proxy
        .call("VadPush", (session, 20u32, vec![0.0f32; 6]))
        .await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "speech_end");
    assert_eq!(
        events[0]["voiced_ms"], 120,
        "voiced time must carry over from the previous call"
    );
    assert_eq!(events[0]["emit"], true);
    assert!(!proxy.call::<bool>("VadIsSpeaking", (session,)).await?);

    proxy.call::<()>("VadClose", (session,)).await?;
    Ok(())
}

#[tokio::test]
async fn frame_indices_are_per_call_not_cumulative() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let session: u64 = proxy.call("VadOpen", (config,)).await?;

    // Ten silent frames, then a push whose second frame opens an utterance.
    let _: String = proxy
        .call("VadPush", (session, 20u32, vec![0.0f32; 10]))
        .await?;
    let json: String = proxy
        .call("VadPush", (session, 20u32, vec![0.0f32, 0.5f32]))
        .await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(
        events[0]["frame"], 1,
        "index is within this call, not a running total"
    );
    Ok(())
}

#[tokio::test]
async fn reset_drops_a_partial_utterance_without_an_event() -> tinybus::Result<()> {
    // The privacy hook. A screen lock must not produce a transcribable segment.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let session: u64 = proxy.call("VadOpen", (config,)).await?;

    let _: String = proxy
        .call("VadPush", (session, 20u32, vec![0.5f32; 30]))
        .await?;
    assert!(proxy.call::<bool>("VadIsSpeaking", (session,)).await?);

    proxy.call::<()>("VadReset", (session,)).await?;
    assert!(!proxy.call::<bool>("VadIsSpeaking", (session,)).await?);
    Ok(())
}

#[tokio::test]
async fn sessions_are_isolated_from_one_another() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let a: u64 = proxy.call("VadOpen", (config.clone(),)).await?;
    let b: u64 = proxy.call("VadOpen", (config,)).await?;
    assert_ne!(a, b);

    let _: String = proxy.call("VadPush", (a, 20u32, vec![0.5f32; 30])).await?;
    assert!(proxy.call::<bool>("VadIsSpeaking", (a,)).await?);
    assert!(
        !proxy.call::<bool>("VadIsSpeaking", (b,)).await?,
        "pushing into one session must not advance another"
    );
    Ok(())
}

#[tokio::test]
async fn a_closed_session_is_refused_rather_than_silently_reopened() -> tinybus::Result<()> {
    // Answering a stale handle by creating a fresh segmenter would look like it
    // worked while silently restarting the utterance.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let session: u64 = proxy.call("VadOpen", (config,)).await?;
    proxy.call::<()>("VadClose", (session,)).await?;

    let result = proxy
        .call::<String>("VadPush", (session, 20u32, vec![0.5f32]))
        .await;
    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "a closed session unexpectedly accepted a push",
        ));
    };
    assert!(error.to_string().contains("is not open"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn closing_twice_is_not_an_error() -> tinybus::Result<()> {
    // Teardown must not itself be able to fail, or hosts start skipping it.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");
    let session: u64 = proxy.call("VadOpen", (config,)).await?;
    proxy.call::<()>("VadClose", (session,)).await?;
    proxy.call::<()>("VadClose", (session,)).await?;
    proxy.call::<()>("VadClose", (9_999_999,)).await?;
    Ok(())
}

#[tokio::test]
async fn ids_are_never_reused() -> tinybus::Result<()> {
    // A reused id lets a stale handle land on the next utterance's segmenter.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");

    let first: u64 = proxy.call("VadOpen", (config.clone(),)).await?;
    proxy.call::<()>("VadClose", (first,)).await?;
    let second: u64 = proxy.call("VadOpen", (config,)).await?;

    assert_ne!(first, second, "a freed id must not come back");
    Ok(())
}

#[tokio::test]
async fn the_session_cap_refuses_new_sessions_rather_than_evicting_old_ones() -> tinybus::Result<()>
{
    // Evicting would silently truncate an utterance somebody is still recording.
    let proxy = connect().await?;
    let config = serde_json::to_string(&tinyvoice::vad::VadConfig::default()).expect("serialize");

    let mut opened = Vec::new();
    for _ in 0..MAX_SESSIONS {
        opened.push(proxy.call::<u64>("VadOpen", (config.clone(),)).await?);
    }
    let result = proxy.call::<u64>("VadOpen", (config,)).await;
    let Err(error) = result else {
        return Err(tinybus::Error::failed("the session cap was not enforced"));
    };
    assert!(error.to_string().contains("session limit"), "got: {error}");

    // The oldest session is still alive, which is the point.
    let _: String = proxy
        .call("VadPush", (opened[0], 20u32, vec![0.5f32]))
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_malformed_config_is_refused_at_open() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<u64>("VadOpen", (r#"{"onset_threshold":0.1}"#,))
        .await;
    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "a partial config unexpectedly opened",
        ));
    };
    assert!(
        error.to_string().contains("invalid VAD config"),
        "got: {error}"
    );
    Ok(())
}

// --- Frame preparation ---

#[tokio::test]
async fn prepare_frames_returns_samples_not_a_container() -> tinybus::Result<()> {
    let proxy = connect().await?;
    // 400 interleaved samples = 200 stereo frames at 32 kHz -> 100 at 16 kHz.
    let stereo: Vec<f32> = (0..400).map(|i| ((i as f32) / 20.0).sin() * 0.5).collect();

    let encoded: String = proxy
        .call("PrepareFrames", (encode_samples(&stereo), 32_000u32, 2u16))
        .await?;
    let bytes = BASE64.decode(&encoded).expect("valid base64");

    assert_eq!(bytes.len(), 100 * 4, "f32 mono samples, no WAV header");
    assert_ne!(&bytes[0..4], b"RIFF");
    Ok(())
}

#[tokio::test]
async fn frame_energies_measures_a_short_trailing_frame_rather_than_dropping_it()
-> tinybus::Result<()> {
    // Dropping it loses the end of an utterance; padding with silence would
    // dilute real speech into a below-threshold frame.
    let proxy = connect().await?;
    let samples = vec![0.5f32; 25];

    let energies: Vec<f32> = proxy
        .call("FrameEnergies", (encode_samples(&samples), 10u32))
        .await?;

    assert_eq!(energies.len(), 3, "two whole frames plus a 5-sample tail");
    for e in &energies {
        assert!(
            (e - 0.5).abs() < 1e-5,
            "constant signal keeps its magnitude: {e}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_zero_frame_length_is_refused() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<Vec<f32>>("FrameEnergies", (encode_samples(&[0.1]), 0u32))
        .await;
    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "zero frame_len unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("frame_len"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn pcm16_encoding_is_exact() -> tinybus::Result<()> {
    // The reason this method exists: a caller holding i16 must get its samples
    // back byte-for-byte, not widened to f32 and narrowed again.
    let proxy = connect().await?;
    let pcm: Vec<i16> = vec![0, 1, -1, i16::MAX, i16::MIN + 1];
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();

    let encoded: String = proxy
        .call("EncodeWavPcm16", (BASE64.encode(&bytes), 16_000u32, 1u16))
        .await?;
    let wav = BASE64.decode(&encoded).expect("valid base64");

    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(wav.len(), 44 + pcm.len() * 2);
    assert_eq!(&wav[44..], &bytes[..], "samples must survive unchanged");
    Ok(())
}

#[tokio::test]
async fn a_truncated_pcm16_buffer_is_refused() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let result = proxy
        .call::<String>("EncodeWavPcm16", (BASE64.encode([0u8; 3]), 16_000u32, 1u16))
        .await;
    let Err(error) = result else {
        return Err(tinybus::Error::failed(
            "an odd byte count unexpectedly succeeded",
        ));
    };
    assert!(error.to_string().contains("i16 samples"), "got: {error}");
    Ok(())
}

#[tokio::test]
async fn native_capture_members_own_resources_and_return_bounded_wav_batches() -> tinybus::Result<()>
{
    use tinyvoice_bus::capture::{
        AudioOutput, CaptureError, CaptureHandle, CaptureResult, ReadAudioRequest,
        RecordingFinishRequest, RecordingStartRequest, RecordingSummary, RecordingSummaryRequest,
    };
    let bus = MemoryBus::new();
    let task = Broker::new().spawn(bus.clone());
    let service = Connection::connect(bus.connect().await?).await?;
    service
        .serve_at(
            OBJECT_PATH.try_into()?,
            VoiceService {
                capture: super::capture::fixture_capture(),
                ..Default::default()
            },
        )
        .await?;
    service.request_name(BUS_NAME).await?;
    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)?;
    let devices: CaptureResult<Vec<String>> =
        proxy.call(names::methods::LIST_INPUT_DEVICES, ()).await?;
    assert_eq!(devices, Ok(vec!["fixture".into()]));
    let denied: CaptureResult<CaptureHandle> = proxy
        .call(
            names::methods::RECORDING_START,
            (RecordingStartRequest::default(),),
        )
        .await?;
    assert_eq!(denied, Err(CaptureError::PermissionDenied));
    let handle: CaptureResult<CaptureHandle> = proxy
        .call(
            names::methods::RECORDING_START,
            (reserved_request(&proxy).await?,),
        )
        .await?;
    let handle = handle.unwrap();
    let output: CaptureResult<AudioOutput> = proxy
        .call(
            names::methods::RECORDING_FINISH,
            (RecordingFinishRequest {
                handle,
                gate_threshold: 0.0,
            },),
        )
        .await?;
    let output = output.unwrap();
    let summary: CaptureResult<RecordingSummary> = proxy
        .call(
            names::methods::RECORDING_SUMMARY,
            (RecordingSummaryRequest {
                handle: output.handle.clone(),
            },),
        )
        .await?;
    let summary = summary.unwrap();
    assert_eq!(summary.sample_count, 320);
    assert!((summary.peak_rms - 0.25).abs() < f32::EPSILON);
    let bytes: CaptureResult<String> = proxy
        .call(
            names::methods::READ_AUDIO_OUTPUT,
            (ReadAudioRequest {
                handle: output.handle.clone(),
                offset: 0,
                length: 256,
            },),
        )
        .await?;
    assert_eq!(BASE64.decode(bytes.unwrap()).unwrap().len(), 256);
    let released: CaptureResult<()> = proxy
        .call(
            names::methods::RELEASE_AUDIO_OUTPUT,
            (output.handle.clone(),),
        )
        .await?;
    assert_eq!(released, Ok(()));
    let missing: CaptureResult<String> = proxy
        .call(
            names::methods::READ_AUDIO_OUTPUT,
            (ReadAudioRequest {
                handle: output.handle,
                offset: 0,
                length: 1,
            },),
        )
        .await?;
    assert_eq!(missing, Err(CaptureError::UnknownHandle));
    let handle: CaptureResult<CaptureHandle> = proxy
        .call(
            names::methods::RECORDING_START,
            (reserved_request(&proxy).await?,),
        )
        .await?;
    let canceled: CaptureResult<()> = proxy
        .call(names::methods::RECORDING_CANCEL, (handle.unwrap(),))
        .await?;
    assert_eq!(canceled, Ok(()));
    task.abort();
    Ok(())
}

#[tokio::test]
async fn prepare_capture_gates_inside_the_module_and_rejects_invalid_formats() -> tinybus::Result<()>
{
    let proxy = connect().await?;
    let wav: String = proxy
        .call(
            names::methods::PREPARE_CAPTURE,
            (encode_samples(&[0.25; 320]), 16_000_u32, 1_u16, 0.01_f32),
        )
        .await?;
    assert_eq!(&BASE64.decode(wav).unwrap()[..4], b"RIFF");
    for (samples, rate, channels) in [
        ("invalid".to_string(), 16_000_u32, 1_u16),
        (encode_samples(&[0.25]), 0, 1),
        (encode_samples(&[0.25]), 16_000, 0),
    ] {
        assert!(
            proxy
                .call::<String>(
                    names::methods::PREPARE_CAPTURE,
                    (samples, rate, channels, 0.0_f32)
                )
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn continuous_capture_lifecycle_executes_through_bus_fixtures() -> tinybus::Result<()> {
    use tinyvoice_bus::capture::*;
    let bus = MemoryBus::new();
    let task = Broker::new().spawn(bus.clone());
    let service = Connection::connect(bus.connect().await?).await?;
    let fixture = VoiceService {
        capture: super::capture::fixture_capture(),
        ..VoiceService::default()
    };
    service.serve_at(OBJECT_PATH.try_into()?, fixture).await?;
    service.request_name(BUS_NAME).await?;
    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME)?;
    let denied: CaptureResult<CaptureStream> = proxy
        .call(
            names::methods::CAPTURE_START,
            (RecordingStartRequest::default(),),
        )
        .await?;
    assert!(matches!(denied, Err(CaptureError::PermissionDenied)));
    let stream: CaptureResult<CaptureStream> = proxy
        .call(
            names::methods::CAPTURE_START,
            (reserved_request(&proxy).await?,),
        )
        .await?;
    let stream = stream.unwrap();
    assert_eq!(stream.format.channels, 1);
    let batch: CaptureResult<CaptureBatch> = proxy
        .call(
            names::methods::CAPTURE_POLL,
            (CapturePollRequest {
                handle: stream.handle.clone(),
                max_chunks: 2,
            },),
        )
        .await?;
    assert_eq!(batch.unwrap().chunks[0].samples, vec![0.25; 16]);
    let stopped: CaptureResult<()> = proxy
        .call(names::methods::CAPTURE_STOP, (stream.handle.clone(),))
        .await?;
    assert_eq!(stopped, Ok(()));
    let missing: CaptureResult<()> = proxy
        .call(names::methods::CAPTURE_STOP, (stream.handle,))
        .await?;
    assert_eq!(missing, Err(CaptureError::UnknownHandle));
    task.abort();
    Ok(())
}
#[tokio::test]
async fn prepare_frames_rejects_expanded_output_before_resampling() {
    let service = VoiceService::default();
    let encoded = encode_samples(&vec![0.25; 200]);
    assert!(service.prepare_frames(encoded, 1, 1).await.is_err());
}

async fn reserved_request(
    proxy: &tinybus::Proxy,
) -> tinybus::Result<tinyvoice_bus::capture::RecordingStartRequest> {
    use tinyvoice_bus::capture::{
        CaptureHandle, CaptureResult, MicrophonePermission, RecordingStartRequest,
    };
    let permission = MicrophonePermission::Granted;
    let handle: CaptureResult<CaptureHandle> = proxy
        .call(
            names::methods::RESERVE_CAPTURE,
            (RecordingStartRequest {
                permission,
                ..Default::default()
            },),
        )
        .await?;
    Ok(RecordingStartRequest {
        permission,
        handle: Some(handle.unwrap()),
    })
}
#[tokio::test]
async fn legacy_prepare_capture_rejects_expansion_before_encoding() {
    let service = VoiceService::default();
    assert!(
        service
            .prepare_capture(super::encode_samples(&vec![0.25; 300]), 1, 1, 0.0)
            .await
            .is_err()
    );
    assert!(super::check_expansion(usize::MAX, 1).is_err());
    assert!(super::check_expansion(MAX_AUDIO_BYTES / 4, tinyvoice::audio::STT_SAMPLE_RATE).is_ok());
}
#[tokio::test]
async fn capture_shutdown_is_terminal_over_the_bus() -> tinybus::Result<()> {
    use tinyvoice_bus::capture::{CaptureError, CaptureResult};
    let proxy = connect().await?;
    let result: CaptureResult<()> = proxy.call(names::methods::CAPTURE_SHUTDOWN, ()).await?;
    assert_eq!(result, Ok(()));
    let reservation: CaptureResult<tinyvoice_bus::capture::CaptureHandle> = proxy
        .call(
            names::methods::RESERVE_CAPTURE,
            (tinyvoice_bus::capture::RecordingStartRequest {
                permission: tinyvoice_bus::capture::MicrophonePermission::Granted,
                ..Default::default()
            },),
        )
        .await?;
    assert_eq!(reservation, Err(CaptureError::Closed));
    Ok(())
}

#[tokio::test]
async fn hotkey_contract_replays_events_and_shutdown_is_terminal() -> tinybus::Result<()> {
    use tinyvoice_bus::{HotkeyError, HotkeyResult};
    let proxy = connect().await?;
    let reserved: HotkeyResult<tinyvoice_bus::HotkeyHandle> = proxy
        .call(
            names::methods::HOTKEY_RESERVE,
            (HotkeyReserveRequest {
                request: HotkeyRequest {
                    key: "Fn".into(),
                    mode: ActivationMode::Push,
                    source: HotkeySource::Host,
                },
            },),
        )
        .await?;
    let handle = reserved.unwrap();
    let started: HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            names::methods::HOTKEY_START,
            (HotkeyHandleRequest {
                handle: handle.clone(),
            },),
        )
        .await?;
    let generation = started.unwrap().generation.unwrap();
    let fed: HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            names::methods::HOTKEY_FEED,
            (HotkeyFeedRequest {
                handle: handle.clone(),
                generation,
                facts: vec![SequencedHostFact {
                    sequence: 1,
                    fact: HostKeyFact::Down,
                }],
                overflow: false,
            },),
        )
        .await?;
    assert!(fed.unwrap().active);
    let read = HotkeyReadRequest {
        handle: handle.clone(),
        acknowledged_batch: None,
    };
    let first: HotkeyResult<tinyvoice_bus::HotkeyBatch> = proxy
        .call(names::methods::HOTKEY_READ, (read.clone(),))
        .await?;
    let first = first.unwrap();
    assert_eq!(first.events[0].event, tinyvoice_bus::HotkeyEvent::Pressed);
    let replay: HotkeyResult<tinyvoice_bus::HotkeyBatch> =
        proxy.call(names::methods::HOTKEY_READ, (read,)).await?;
    assert_eq!(replay.unwrap(), first);
    let stopped: HotkeyResult<tinyvoice_bus::HotkeyReply> = proxy
        .call(
            names::methods::HOTKEY_STOP,
            (HotkeyHandleRequest { handle },),
        )
        .await?;
    assert_eq!(stopped.unwrap().state, tinyvoice_bus::HotkeyState::Stopped);
    let shutdown: HotkeyResult<tinyvoice_bus::HotkeyReply> =
        proxy.call(names::methods::HOTKEY_SHUTDOWN, ()).await?;
    assert_eq!(shutdown.unwrap().state, tinyvoice_bus::HotkeyState::Stopped);
    let after: HotkeyResult<tinyvoice_bus::HotkeyHandle> = proxy
        .call(
            names::methods::HOTKEY_RESERVE,
            (HotkeyReserveRequest {
                request: HotkeyRequest {
                    key: "Fn".into(),
                    mode: ActivationMode::Tap,
                    source: HotkeySource::Host,
                },
            },),
        )
        .await?;
    assert_eq!(after, Err(HotkeyError::Closed));
    Ok(())
}

#[tokio::test]
async fn relay_preparation_preserves_text_history_and_readback_boundaries() {
    let proxy = connect().await.unwrap();
    let request = serde_json::json!({
        "messages": [
            {"role": "system", "content": "provider guidance"},
            {"role": "user", "content": "earlier question"},
            {"role": "agent", "content": "earlier answer"},
            {"role": "tool", "content": "private tool output"},
            {"role": "user", "content": [
                {"type": "text", "text": "  Read verbatim:"},
                {"type": "image", "url": "ignored"},
                {"type": "text", "text": "  hello 世界  "}
            ]}
        ],
        "readback_prefix": "Read verbatim:"
    });
    let reply: serde_json::Value = proxy.call("PrepareRelayTurn", &request).await.unwrap();
    assert_eq!(reply["prompt"], "  Read verbatim:   hello 世界  ");
    assert_eq!(reply["readback_payload"], "hello 世界");
    assert_eq!(reply["content_free"], false);
    assert_eq!(reply["answerable"], false);
    assert_eq!(reply["arm_speak_back"], false);
    assert_eq!(
        reply["history"],
        serde_json::json!([
            ["user", "earlier question"],
            ["agent", "earlier answer"],
            ["user", "  Read verbatim:   hello 世界  "]
        ])
    );
}

#[tokio::test]
async fn relay_rejects_aggregate_text_before_amplifying_a_reply() {
    let request = tinyvoice_bus::relay::RelayTurnRequest {
        messages: vec![serde_json::json!({"role":"user","content":"x".repeat(8 * 1024 * 1024)})],
        readback_prefix: String::new(),
    };
    let result = super::VoiceService::default()
        .prepare_relay_turn(request)
        .await;
    assert!(
        result.is_err(),
        "an accepted single-frame request must not amplify into an oversized reply"
    );
}

#[tokio::test]
async fn oversized_relay_request_returns_an_error_without_breaking_the_bus() {
    let proxy = connect().await.unwrap();
    let request = tinyvoice_bus::relay::RelayTurnRequest {
        messages: vec![serde_json::json!({"role":"user","content":"x".repeat(8 * 1024 * 1024)})],
        readback_prefix: String::new(),
    };
    let rejected: tinybus::Result<serde_json::Value> =
        proxy.call("PrepareRelayTurn", &request).await;
    assert!(rejected.is_err());
    let reply: serde_json::Value = proxy
        .call(
            "PrepareRelayTurn",
            serde_json::json!({
                "messages":[{"role":"user","content":"hello"}], "readback_prefix":"Read:"
            }),
        )
        .await
        .unwrap();
    assert_eq!(reply["prompt"], "hello");
}

#[tokio::test]
async fn live_first_frame_parsing_preserves_default_rate_and_host_metadata() {
    let proxy = connect().await.unwrap();
    let reply: serde_json::Value = proxy.call("PrepareLiveInput", serde_json::json!({
        "awaiting_start": true,
        "frame": {"kind": "text", "text": "{\"type\":\"start\",\"thread_id\":\"thread\",\"client_id\":\"client\"}"}
    })).await.unwrap();
    assert_eq!(reply["type"], "start");
    assert_eq!(reply["input_sample_rate"], 16000);
    assert_eq!(reply["thread_id"], "thread");
    assert_eq!(reply["client_id"], "client");
}

#[tokio::test]
async fn live_reservations_exist_before_provider_startup_and_can_be_released() {
    let proxy = connect().await.unwrap();
    let reserved: serde_json::Value = proxy.call("LiveReserve", ()).await.unwrap();
    assert_eq!(reserved["state"], "reserved");
    assert_eq!(reserved["handle"]["id"].as_str().unwrap().len(), 32);
    let released: serde_json::Value = proxy
        .call(
            "LiveRelease",
            serde_json::json!({
                "handle": reserved["handle"],
            }),
        )
        .await
        .unwrap();
    assert_eq!(released["state"], "closed");
}

#[tokio::test]
async fn live_callback_access_does_not_retain_the_module_bus_name_after_drop() {
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let service = Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    setup(service.clone()).await.unwrap();
    let client = Connection::connect(bus.connect().await.unwrap())
        .await
        .unwrap();
    let proxy = client.proxy(BUS_NAME, OBJECT_PATH, BUS_NAME).unwrap();
    let reserved: serde_json::Value = proxy.call("LiveReserve", ()).await.unwrap();
    assert_eq!(reserved["state"], "reserved");
    drop(service);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if !client
                .list_names()
                .await
                .unwrap()
                .iter()
                .any(|name| name.as_str() == BUS_NAME)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping the module owner must release its name");
}

#[tokio::test]
async fn bound_live_transport_is_idempotent_and_excludes_competing_manual_readers() {
    let proxy = connect().await.unwrap();
    let reserved: serde_json::Value = proxy.call("LiveReserve", ()).await.unwrap();
    let request = serde_json::json!({
        "handle": reserved["handle"],
        "port": {"bus_name":"ai.test.Transport", "object_path":"/test/Transport", "context_token":"known"}
    });
    let first: serde_json::Value = proxy.call("LiveBindTransport", &request).await.unwrap();
    assert_eq!(first["state"], "reserved");
    let repeated: serde_json::Value = proxy.call("LiveBindTransport", &request).await.unwrap();
    assert_eq!(first, repeated);
    let rejected: tinybus::Result<serde_json::Value> = proxy
        .call(
            "LiveRead",
            serde_json::json!({
                "handle":reserved["handle"], "acknowledged_batch":null
            }),
        )
        .await;
    assert!(rejected.is_err());
    let changed = serde_json::json!({"handle":reserved["handle"],"port":{
        "bus_name":"ai.test.Transport","object_path":"/test/Transport","context_token":"different"
    }});
    let rejected: tinybus::Result<serde_json::Value> =
        proxy.call("LiveBindTransport", changed).await;
    assert!(rejected.is_err());
    // This fixture deliberately has no callback endpoint. Teardown fails
    // boundedly rather than pretending the host acknowledged socket cleanup.
    let _: tinybus::Result<serde_json::Value> = proxy
        .call(
            "LiveRelease",
            serde_json::json!({"handle":reserved["handle"]}),
        )
        .await;
}

#[tokio::test]
async fn relay_limits_are_configured_once_before_lease_admission() {
    let proxy = connect().await.unwrap();
    let limits = serde_json::json!({"max_active_turns":3,"max_pending_turns":16});
    let first: serde_json::Value = proxy.call("RelayConfigure", &limits).await.unwrap();
    assert_eq!(first, limits);
    let repeated: serde_json::Value = proxy.call("RelayConfigure", &limits).await.unwrap();
    assert_eq!(repeated, first);
    let changed: tinybus::Result<serde_json::Value> = proxy
        .call(
            "RelayConfigure",
            serde_json::json!({"max_active_turns":2,"max_pending_turns":16}),
        )
        .await;
    assert!(changed.is_err());
    let lease: serde_json::Value = proxy.call("RelayReserve", ()).await.unwrap();
    assert_eq!(lease["state"], "reserved");
    assert_eq!(lease["handle"]["id"].as_str().unwrap().len(), 32);
}

#[tokio::test]
async fn sequence_utterance_reports_the_disposition() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let command: String = proxy
        .call("SequenceUtterance", ("hey tiny open slack", "Hey Tiny"))
        .await?;
    assert_eq!(
        command,
        r#"{"disposition":"command","command":"open slack"}"#
    );
    let bare: String = proxy
        .call("SequenceUtterance", ("hey tiny", "Hey Tiny"))
        .await?;
    assert_eq!(bare, r#"{"disposition":"bare_wake"}"#);
    let ignored: String = proxy
        .call("SequenceUtterance", ("open slack", "Hey Tiny"))
        .await?;
    assert_eq!(ignored, r#"{"disposition":"ignored"}"#);
    Ok(())
}

#[tokio::test]
async fn gate_recording_applies_both_thresholds() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let short: String = proxy
        .call("GateRecording", (0.1f32, 0.5f32, 0.3f32, 0.002f32))
        .await?;
    assert_eq!(short, r#""too_short""#);
    let pass: String = proxy
        .call("GateRecording", (1.0f32, 0.5f32, 0.3f32, 0.002f32))
        .await?;
    assert_eq!(pass, r#""pass""#);
    Ok(())
}

#[tokio::test]
async fn recording_session_drives_press_to_finish_over_the_bus() -> tinybus::Result<()> {
    let proxy = connect().await?;
    let id: u64 = proxy.call("SessionOpen", ()).await?;
    let advance = async |input: &str| -> tinybus::Result<serde_json::Value> {
        let reply: String = proxy
            .call("SessionAdvance", (id, input.to_string()))
            .await?;
        Ok(serde_json::from_str(&reply).unwrap())
    };
    assert!(
        advance(r#"{"input":"run"}"#).await?["accepted"]
            .as_bool()
            .unwrap()
    );
    let started = advance(r#"{"input":"pressed"}"#).await?;
    assert_eq!(started["action"]["action"], "start_recording");
    assert_eq!(started["phase"], "recording");
    let live = advance(r#"{"input":"setup_ready","stop_buffered":false}"#).await?;
    assert_eq!(live["action"]["action"], "recording_live");
    let stopped = advance(r#"{"input":"released"}"#).await?;
    assert_eq!(stopped["action"]["action"], "stop_recording");
    let generation = stopped["generation"].as_u64().unwrap();
    let transcribing = advance(&format!(
        r#"{{"input":"transcribing","generation":{generation}}}"#
    ))
    .await?;
    assert_eq!(transcribing["phase"], "transcribing");
    let stale = advance(r#"{"input":"finished","generation":999}"#).await?;
    assert!(!stale["accepted"].as_bool().unwrap());
    let status: String = proxy.call("SessionStatus", (id,)).await?;
    assert_eq!(
        status,
        format!(r#"{{"phase":"transcribing","generation":{generation}}}"#)
    );
    proxy.call::<()>("SessionClose", (id,)).await?;
    proxy.call::<()>("SessionClose", (id,)).await?;
    assert!(proxy.call::<String>("SessionStatus", (id,)).await.is_err());
    Ok(())
}

#[tokio::test]
async fn recording_session_rejects_unknown_ids_and_bad_input() -> tinybus::Result<()> {
    let proxy = connect().await?;
    assert!(
        proxy
            .call::<String>(
                "SessionAdvance",
                (9_999u64, r#"{"input":"run"}"#.to_string())
            )
            .await
            .is_err()
    );
    let id: u64 = proxy.call("SessionOpen", ()).await?;
    assert!(
        proxy
            .call::<String>("SessionAdvance", (id, "not json".to_string()))
            .await
            .is_err()
    );
    Ok(())
}
