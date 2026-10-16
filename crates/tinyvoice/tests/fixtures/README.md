# Codec test fixtures

`voice-pop.wav` is the 268-byte mono PCM fixture `testsamples/pop.wav` from
the `hound` 3.5.1 crate source distribution. It is a short real audio signal
used to exercise decoding and encoder interoperability without network access.
The hound crate and its sample are distributed under MIT; see the `license`
file in the [hound 3.5.1 source](https://crates.io/crates/hound/3.5.1).

`mp3-test.bit` is the Layer III test stream `vectors/l3-si.bit` from
[minimp3](https://github.com/lieff/minimp3), checked in as a source-format
fixture to cover common `audio/mpeg` text-to-speech responses. The upstream
repository dedicates its contents to CC0; the notice is included as
[`CC0-1.0.txt`](CC0-1.0.txt).
