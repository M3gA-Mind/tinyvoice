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

`amr/amrnb_mode0.amr` and `amr/amrnb_mode0_reference.pcm` are the mode-0
AMR-NB storage stream and expected decoded PCM from
[`rvoip-codec-core` 0.4.0](https://github.com/voicetastic/rvoip), pinned at
source commit `f4d528eaabbac935bce9a6404d3ce4fdec35ed0f`. The upstream tests
identify the PCM as output from the 3GPP TS 26.073 reference decoder. The
published crate is MIT-licensed; this test-only fixture is retained with that
attribution. The AMR file is 331 bytes and the PCM is 8,000 bytes (400 ms of
8 kHz mono signed 16-bit samples).
