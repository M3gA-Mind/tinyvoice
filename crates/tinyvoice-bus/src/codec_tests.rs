use super::*;

#[test]
fn amr_nb_codec_tag_round_trips_in_transcode_requests() {
    let wire = serde_json::json!({
        "audio_base64": "AA==",
        "from": "wav_pcm16",
        "target": "amr_nb"
    });

    let request: CodecTranscodeRequest =
        serde_json::from_value(wire.clone()).expect("AMR-NB wire tag should decode");

    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}
