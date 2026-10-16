# Voice-note codecs

The `tinyvoice::codec` library API accepts complete in-memory byte slices. It
detects WAV/PCM16, Ogg/Opus, or M4A/AAC-LC input, and exposes `probe`,
`transcode`, and `waveform`. It performs no filesystem, network, process, or
device I/O. Decoding is bounded to 32 MiB of input and five minutes of audio.
Voice encodes are mono 48 kHz; Opus uses the VoIP profile at 24 kbps and AAC-LC
uses 64 kbps. WAV-to-WAV passthrough preserves the original bytes.

Opus encoding, decoding, and Ogg framing use `opus-pure`; M4A demux and AAC
decode use Symphonia; AAC-LC encoding uses `oxideav-aac`; and the `mp4` crate
muxes AAC access units. These are Rust dependencies enabled by default in the
library, so codec support is available without a system FFmpeg install or
native codec library. This increases the linked size of consumers that include
the codec API; measure the final executable for each product profile before
setting a package-size budget. The codecs remain in the `tinyvoice` library,
while module hosts call them through `CodecProbe`, `CodecTranscode`, and
`CodecWaveform` in the minimal `tinyvoice-bus` contract.

`tinyvoice-module` transports complete media as Base64 and caps each request
and output at 8 MiB to stay within bus-message limits. The library itself
accepts up to 32 MiB for non-bus consumers. Waveforms contain one normalized
peak byte per requested bucket, with a maximum of 4096 buckets; 256 buckets
match Discord's voice-message waveform shape.

Integration tests use the checked-in sample documented in
[`tests/fixtures/README.md`](../../crates/tinyvoice/tests/fixtures/README.md),
round-trip through both encoded formats, decode Opus with a second pure-Rust
implementation and M4A with Symphonia, and exercise truncation and bucket
bounds. CI should build the library and module on Windows, macOS, and Linux.
