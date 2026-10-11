# Compiled live voice runtime

The voice module owns portable live connection and duplex session machinery.
It uses TinyAgentsLive's existing provider/tool loop rather than implementing
another agent loop. A host supplies prepared provider credentials, an explicit
live configuration and tool declarations. Provider ticket minting, credential
lookup and provider selection remain host capabilities; the module never calls
a product backend to obtain them.

The bus vocabulary preserves the existing live websocket JSON frames: start,
text, interrupt and stop inputs; ready, transcript, tool_started, tool_finished,
interrupted, turn_complete, error and closed outputs. Audio remains PCM16.
Final transcripts emit their client frame before a separate persistence
projection. The host applies that projection to its own transcript store.
Ready uses the provider's session id when present and the host fallback
otherwise. Terminal error closes emit error before closed. Unknown provider
events produce no invented client event.

A live open request supplies an opaque tool callback endpoint and host context
token. The module builds a TinyAgentsLive harness over callback tools. The host
callback invokes its existing current policy-aware harness with its saved
approval, origin and thread context. A tool declaration grants no permission.
Neither this module nor the callback may fabricate an executor for an unknown
tool or restore a revoked tool. Provider cancellation and tool timeouts retain
the TinyAgentsLive semantics.

Session handles are opaque and random. A bounded table owns sessions and their
tasks. Audio and serialized control requests have bounded sizes. Event reads
return ordered bounded batches with replay until acknowledgment, so a lost
reply does not erase transcript or terminal events. Stop asks the provider to
close and retains its terminal events; explicit close drops the session and
its tasks. Shutdown cancels and joins owned tasks. A disconnected client must
explicitly close its handle; capacity limits prevent leaked clients from
growing the table indefinitely.

The realtime relay's local brain, progress selection, correlation admission,
spoken acknowledgment deadline and delivery policy remain host choices.
Portable turn preparation already uses PrepareRelayTurn. A reusable relay
coordinator must accept explicit host timing and completion callbacks and
preserve delta-before-done and deferred-result delivery ordering; it may not
select a product model, thread, acknowledgment phrase or destination itself.

Tests must exercise actual compiled module operations, a local scripted
provider and a real bus callback. Cover initial frame validation, all existing
event projections, final transcript order, bounded input/event queues, retry
batches, interruptions, cancellation, callback failures, remote closure,
explicit close and shutdown. No test calls an external provider. Host adoption
waits for accepted source and an available released module.
