# Codec test fixtures

`voice-envelope.wav` is a deterministic, locally generated 2.371-second mono
PCM voice-band signal. Its harmonics and alternating voiced/silent sections
exercise multiframe encoding, duration clipping, and envelope retention without
network access or third-party audio provenance.

`opus-interop.wav` is a deterministic, locally generated 2.5-millisecond mono
PCM tone used to exercise the independent Opus decoder against one encoder
packet. Longer multiframe Opus/AAC paths use `voice-envelope.wav`.

`mp3-test.bit` is the Layer III test stream `vectors/l3-si.bit` from
[minimp3](https://github.com/lieff/minimp3), checked in as a source-format
fixture to cover common `audio/mpeg` text-to-speech responses. The upstream
repository dedicates its contents to CC0; the notice is included as
[`CC0-1.0.txt`](CC0-1.0.txt).
