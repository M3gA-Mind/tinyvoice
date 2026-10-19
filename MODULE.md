# TinyVoice TinyBus Module

This package contains the native `tinyvoice` module for TinyBus module ABI v1.
Install only the archive matching the host operating system and architecture.

The module claims `ai.tinyhumans.tinyvoice.Voice`, serves the object at
`/ai/tinyhumans/tinyvoice/Voice`, and provides 56 methods under contract 1.12.

## Methods

| Method | Arguments | Returns |
| --- | --- | --- |
| `CodecProbe` | `CodecInput { audio_base64 }` | `CodecInfo`: detected codec, duration, sample rate, channels |
| `CodecTranscode` | `CodecTranscodeRequest { audio_base64, from, target }` | `CodecOutput { audio_base64 }` |
| `CodecWaveform` | `WaveformRequest { audio_base64, buckets }` | `WaveformResult { peaks }` |
| `Route` | `transcript` | JSON `VoiceIntent`, tagged by an `intent` field |
| `ExtractCommand` | `transcript`, `wake_word` | The command after the wake word, or `""` |
| `WakeWordPresent` | `transcript`, `wake_word` | `bool` |
| `IsHallucinated` | `text`, `mode` | `bool` |
| `GateRecording` | `duration_secs`, `peak_rms`, `min_duration_secs`, `silence_threshold` | JSON `GateVerdict` |
| `SequenceUtterance` | `transcript`, `wake_word` | JSON `UtteranceDisposition` |
| `SessionOpen` | none | recording session id (starts `stopped`) |
| `SessionAdvance` | `session`, JSON `SessionInput` (`run`, `stop`, `pressed`, `released`, `setup_ready`, `setup_failed`, `deferred_stop_elapsed`, `transcribing`, `finished`) | JSON `SessionAdvance`: `accepted`, `phase`, `generation` and the one `action` the host takes (`start_recording`, `recording_live`, `defer_stop`, `stop_recording`, `none`) |
| `SessionStatus` | `session` | JSON `SessionStatus` (`phase`, `generation`) |
| `SessionClose` | `session` | — |
| `Segment` | `config`, `frame_ms`, `energies` | JSON array of frame-indexed VAD events (one-shot) |
| `VadOpen` | `config` | session id |
| `VadPush` | `session`, `frame_ms`, `energies` | JSON array of frame-indexed VAD events |
| `VadIsSpeaking` | `session` | `bool` |
| `VadReset` | `session` | — |
| `VadClose` | `session` | — |
| `PrepareRelayTurn` | `RelayTurnRequest { messages, readback_prefix }` | `PreparedRelayTurn`: prompt, ordered history pairs, optional readback payload, content-free, speak-back and answerable flags |
| `PrepareLiveInput` | `PrepareLiveInputRequest { awaiting_start, frame }` | parsed control or transport action; host owns policy validation |
| `LiveBindTransport` | reserved handle and mechanical host transport port | idempotent `LiveStatus`; module owns the client/provider coordinator |
| `RelayConfigure` | explicit concurrent/pending turn limits | immutable idempotent `RelayLimits` |
| `RelayReserve` | none | bounded opaque `RelayStatus` before native work |
| `RelayStart` | reserved handle, saved host callback context, explicit timing/fallbacks | asynchronous idempotent `RelayStatus` |
| `RelayRelease` | known relay handle | bounded cleanup and closed status |
| `RelayShutdown` | none | permanent closed admission and bounded concurrent cleanup |
| `LiveReserve` | none | opaque `LiveStatus` reserved before connection startup |
| `LiveStart` | `LiveStartRequest` | idempotent starting/running status for identical inputs |
| `LiveSend` | handle and text, PCM16 audio, interrupt or stop command | status after bounded input admission |
| `LiveRead` | handle and optional acknowledged batch | bounded replayable ordered frames and persistence effects |
| `LiveRelease` | handle | closed status after bounded provider/context cleanup |
| `LiveShutdown` | none | terminal admission shutdown and concurrent bounded cleanup |
| `PrepareFrames` | `samples`, `source_rate`, `channels` | base64 `f32` mono @ 16 kHz |
| `FrameEnergies` | `samples`, `frame_len` | per-frame RMS |
| `EncodeWav` | `samples` (`f32` mono), `sample_rate` | base64 WAV |
| `EncodeWavPcm16` | `samples` (`i16`), `sample_rate`, `channels` | base64 WAV, samples unchanged |
| `PrepareCapture` | `samples`, `source_rate`, `channels`, `gate_threshold` | base64 WAV |
| `ListInputDevices` | none | structured result of device names |
| `CaptureShutdown` | none | await native shutdown, terminal for this instance |
| `ReserveCapture` | permission request | known reservation handle |
| `RecordingStart` | permission and reserved handle | opaque recording handle |
| `RecordingFinish` | handle, gate threshold in one request | bounded WAV output handle and length |
| `RecordingSummary` | finished output handle | ungated peak RMS and gated WAV sample count |
| `RecordingCancel` | handle | structured result of unit |
| `ReadAudioOutput` | handle, offset, length in one request | base64 WAV slice |
| `ReleaseAudioOutput` | handle | structured result of unit |
| `CaptureStart` | permission and reserved handle | opaque stream handle and native format |
| `CapturePoll` | handle, max_chunks in one request | ordered raw chunks and closed flag |
| `CaptureStop` | handle | structured result after native thread shutdown |
| `HotkeyReserve` | bounded `HotkeyRequest` | opaque `HotkeyHandle` ([lifecycle](docs/specs/hotkey-module-lifecycle.md)) |
| `HotkeyStart` | reserved handle | status and host-feed generation ([lifecycle](docs/specs/hotkey-module-lifecycle.md)) |
| `HotkeyRead` | handle and optional acknowledged batch | bounded replayable event batch ([lifecycle](docs/specs/hotkey-module-lifecycle.md)) |
| `HotkeyFeed` | handle, generation, sequence, key facts, and overflow flag | feed status ([lifecycle](docs/specs/hotkey-module-lifecycle.md)) |
| `HotkeyStop` | handle | status after listener cleanup ([lifecycle](docs/specs/hotkey-module-lifecycle.md)) |
| `HotkeyShutdown` | none | terminal cleanup status ([lifecycle](docs/specs/hotkey-module-lifecycle.md)) |

Notes on the contract:

- `PrepareRelayTurn` accepts provider-style message JSON and a host-selected
  readback prefix. It preserves supported text and user/assistant/agent history,
  skips unsupported content, and performs no agent execution. Serialized requests
  are limited to 4 MiB in aggregate, including JSON escaping and all fields.
  The limit is checked before constructing copies in the reply. Prepared replies
  also reserve 64 KiB for the envelope within TinyBus's 16 MiB frame limit.
  Oversized calls return a method error and leave the connection usable. See
  [relay preparation](docs/specs/relay-turn-preparation.md) for exact semantics.

- `mode` is `"dictation"` or `"conversation"`. An unrecognised value is an
  **error**, not a silent fallback: the two modes disagree about whether a bare
  "okay" is real speech, so guessing would either delete legitimate chat replies
  or leak artefacts into dictated text.
- `ExtractCommand` returns `""` both when the utterance was not addressed to the
  agent and when the wake word arrived with nothing after it. Those are the same
  outcome for a caller — there is nothing to route either way.
- `Route` never fails on an unrecognised transcript; it answers
  `{"intent":"unknown"}`. Routing may only ever shortcut, never block.
- `samples` are base64 little-endian `f32`. Payloads are capped at 8 MiB, which
  is roughly eight minutes of 16 kHz mono, and the cap is checked *before*
  decoding so an oversized value returns an error rather than an allocation
  failure.
- `Segment` is the **one-shot** form: it starts a fresh segmenter per call and
  drops it afterwards, so submit whole utterances — a batch that cuts one in
  half loses the open segment. A live capture loop should use a session
  (`VadOpen` … `VadPush` … `VadClose`) instead, which carries state across
  calls. A call costs ~13 µs, so per-frame pushes are affordable.
- Sessions are capped at 64 and ids are never reused. Over the cap, `VadOpen`
  refuses rather than evicting — evicting would silently truncate an utterance
  somebody is still recording. `VadClose` on an unknown id is deliberately
  **not** an error, so teardown cannot itself fail.
- `VadPush` frame indices are relative to *that call*, not a running total.

Native capture operations return `CaptureResult` values rather than transport
errors. They require an explicit microphone grant from the host; denial never
opens a device. One active recording or stream owns the microphone slot.
`RecordingSummary` returns only module-computed metrics: ungated peak RMS for
the existing silence policy and sample count for the prepared WAV. Audio
samples remain inside the module unless the host explicitly reads a bounded
WAV slice.
Continuous polling returns at most two chunks from an eight-chunk queue, each
containing at most 32,768 interleaved f32 samples. Call `CaptureStop` even after
the queue closes to release the lease and observe terminal device errors. See
[the capture specification](docs/specs/module-capture.md) for all resource bounds.

## Installing

The archive contains one `.so`, `.dylib`, or `.dll` plus `modules.toml`. Keep
those files together when copying them into a TinyBus module directory. The
allowlist binds the native library filename to its SHA-256 digest so TinyBus can
reject a missing, renamed, or modified artifact before initialization.

The GitHub release also publishes `checksum.toml` as a separate asset. TinyBus
checks that manifest before downloading and extracting the selected platform
archive. Install directly from a tagged release with:

```sh
tinybus modules load-github \
  https://github.com/tinyhumansai/tinyvoice/releases/tag/v0.1.0 \
  tinyvoice-module-0.1.0-ubuntu-24.04-x86_64.tar.gz \
  <archive-sha256>
```

TinyBus modules are trusted in-process code: a loaded module shares the address
space, the privileges and the crash domain, and TinyBus never unloads one.
Install release artifacts only from a trusted source, and restart the host after
replacing a loaded module.

Relay coordination uses context-bound host callbacks; see the [relay runtime contract](docs/specs/relay-module-runtime.md).
