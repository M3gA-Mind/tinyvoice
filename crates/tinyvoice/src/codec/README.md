# Voice codec module

`mod.rs` owns the pure, in-memory public API for probing, transcoding, and
waveform extraction. It detects WAV/PCM16, Ogg/Opus, M4A/AAC-LC, and MP3 input;
transcode callers also provide the source format so mismatches are rejected.
MP3 is decode-only. Compressed destinations are encoded as mono 48 kHz; WAV to
WAV preserves the bytes.

`codec_tests.rs` covers internal format constraints and malformed muxer data.
The public integration coverage is in `tests/codecs.rs`, including fixture
provenance and interoperability checks in `tests/fixtures/README.md`.

Codec operations use pure-Rust dependencies: `opus-pure` for Opus/Ogg,
Symphonia for AAC/M4A, MP3, and WAV decoding, `oxideav-aac` for AAC-LC encoding,
and `mp4` for in-memory M4A muxing. Input is limited to 32 MiB; decoded audio is
limited to five minutes and 32 million samples, with sample rates between 8 and
192 kHz. No codec path uses filesystem, network, process, or device I/O.

The host-facing surface is the typed `tinyvoice-bus` contract and its compiled
`tinyvoice-module` adapter. Hosts must not call this implementation crate
directly for module-owned behavior. See the [codec specification](../../../../docs/specs/voice-codecs.md)
for backend and bus details.
