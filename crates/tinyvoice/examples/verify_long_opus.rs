use std::fs;
use tinyvoice::codec::{self, AudioCodec};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = include_bytes!("../tests/fixtures/voice-envelope.wav");
    let opus = codec::transcode(input, AudioCodec::WavPcm16, AudioCodec::OggOpus)?;
    let baseline = codec::transcode(input, AudioCodec::WavPcm16, AudioCodec::WavPcm16)?;
    fs::write("target/voice-envelope.ogg", opus)?;
    fs::write("target/voice-envelope.wav", baseline)?;
    Ok(())
}
