# Cloud voice providers

`CloudVoiceProvider` is the transport-free interface between a host’s voice
selection and its backend layer. A backend implementation receives encoded audio,
reply text or a realtime bootstrap request; it owns credentials, endpoint choice,
HTTP and backend error classification. Neither requests nor the voice selector
need a backend base URL or a persistent credential.

The trait returns boxed `Send` futures using the standard library. Hosts choose
their runtime and install providers through their own composition mechanism.
Transcription and session replies use typed DTOs. Speech synthesis preserves the
provider’s JSON response so a host can retain its established tolerant alignment
and viseme normalization. Provider errors pass through unchanged, including any
host authentication-expiry markers.

This interface does not add TinyBus methods or raise the native wire contract
version. It does not implement capture, VAD, hotkeys, playback, provider websocket
protocols or local speech engines. Those operations retain their existing owner
and compiled-module execution paths. A provider implementation must not replace
the native module with host-side voice processing.

The public contract test implements a fake host provider and exercises all
operations, including transcription failure, without network access or a runtime.
