# Host-supplied cloud voice transport

OpenHuman’s cloud transcription, reply speech and hosted realtime bootstrap need
a shared transport-free interface so their backend layer can own HTTP and
credential handling while the voice domain keeps selection and local fallbacks.

The additive `tinyvoice-bus::provider` contract accepts encoded audio and optional
transcription hints, reply text and optional speech hints, or a typed session
bootstrap request. It returns transcription text, an opaque speech response, or
short-lived connection facts. Object-safe futures use the standard library;
errors preserve typed categories independently of display text. Debug output
redacts realtime connection credentials while serialization retains them. No persistent credential or backend base
URL appears in the input types.

Speech response normalization and generic websocket/provider execution keep their
existing owners. The interface neither supplies native voice processing nor adds
TinyBus methods. Existing capture, hotkey and VAD behavior remains behind the
compiled module; its wire contract and published native artifacts remain valid.

Deterministic public tests must prove trait-object dispatch for every operation,
failed transcription, and serialization preserving optional hints and session
metadata. All tests use fake providers and no network or clock access.

Implementation order: [cloud provider plan](../plans/cloud-provider.md).
