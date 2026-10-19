//! Stable wire tags for codec requests crossing the module boundary.

use tinyvoice_bus::CodecTranscodeRequest;

#[test]
fn amr_nb_codec_tag_round_trips_in_transcode_requests() -> Result<(), Box<dyn std::error::Error>> {
    let wire = serde_json::json!({
        "audio_base64": "AA==",
        "from": "wav_pcm16",
        "target": "amr_nb"
    });

    let request: CodecTranscodeRequest = serde_json::from_value(wire.clone())?;

    assert_eq!(serde_json::to_value(request)?, wire);
    Ok(())
}
