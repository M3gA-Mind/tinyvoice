# Voice-note codecs

The `tinyvoice::codec` library API accepts complete in-memory byte slices. It
detects WAV/PCM16, Ogg/Opus, M4A/AAC-LC, MPEG Layer III, and (with the
`amr-nb` feature) AMR-NB storage input, and exposes
`probe`, `transcode(bytes, from, to)`, and `waveform`. The declared source
format is checked against the input bytes; MP3 is decode-only. It performs no
filesystem, network, process, or device I/O. Decoding is bounded to 32 MiB of
input, five minutes of audio, and 32 million decoded samples. Sample rates
outside 8–192 kHz are rejected.
Opus and AAC voice encodes are mono 48 kHz; Opus uses the VoIP profile at
24 kbps and AAC-LC uses 64 kbps. AMR-NB uses mono 8 kHz, 20 ms frames, and the
RFC 4867 storage format (`#!AMR\n`); its measured duration comes from decoded
frames. WAV-to-WAV passthrough preserves the original bytes.

Opus encoding, decoding, and Ogg framing use `opus-pure`; M4A demux and AAC
decode use Symphonia; AAC-LC encoding uses `oxideav-aac`; and the `mp4` crate
muxes AAC access units. These are Rust dependencies enabled by default in the
library, so codec support is available without a system FFmpeg install or
native codec library. The optional `amr-nb` feature uses the pure-Rust
`rvoip-codec-core` implementation and raises the library and module minimum
Rust version to 1.91. A Linux x86_64 release build of `tinyvoice-module`
measured 20.8 MiB total / 7.8 MiB `.text` with `cargo bloat`; its report
attributed 618.7 KiB `.text` to `opus-pure` and 118.7 KiB to `oxideav-aac`
(Symphonia's decoder is split across several crates). This is the complete
module size, not the codec-only delta; measure product artifacts before
setting a package-size budget. The codecs remain in the `tinyvoice` library,
while module hosts call them through `CodecProbe`, `CodecTranscode`, and
`CodecWaveform` in the minimal `tinyvoice-bus` contract.

`tinyvoice-module` transports complete media as Base64 and caps each request
and output at 8 MiB to stay within bus-message limits. The library itself
accepts up to 32 MiB for non-bus consumers. Waveforms contain one normalized
peak byte per requested bucket, with a maximum of 4096 buckets; 256 buckets
match Discord's voice-message waveform shape.

Integration tests use the checked-in samples documented in
[`tests/fixtures/README.md`](../../crates/tinyvoice/tests/fixtures/README.md),
round-trip through encoded formats, compare AMR-NB against pinned 3GPP reference
storage and PCM vectors, decode Opus with a second pure-Rust implementation and
M4A with Symphonia, and exercise truncation and bucket bounds. CI builds the
AMR-enabled library and module on the declared Rust 1.91 minimum, then covers
the supported platform matrix.
