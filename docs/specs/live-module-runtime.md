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

A reserved live start request supplies an opaque tool callback endpoint and host context
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
close and retains its terminal events; explicit release drops the session and
its tasks. Shutdown closes admission, cancels every native task and joins cleanups concurrently under one two-second deadline. Dropping a release/shutdown future aborts the worker rather than detaching its task. Module drop also aborts owned workers. A disconnected client must
explicitly release its handle; capacity limits prevent leaked clients from
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

## Admission and transport bounds

Contract 1.7 adds PrepareLiveInput, LiveReserve, LiveStart, LiveSend, LiveRead,
LiveRelease and LiveShutdown. Reserve precedes provider connection; identical
start retries preserve the same lease and a changed start request fails.
Sixteen leases are admitted. Unstarted reservations expire after sixty seconds;
active sessions are never evicted to admit another client. Startup requests
and tool callback requests/replies each have a one-MiB serialized limit.
Text/audio input is at most sixty-four KiB (PCM16 audio contains whole samples).
A command waits at most one second for native input admission. Connection
timeouts are at most sixty seconds and tool timeouts at most thirty minutes;
both and the maximum tool-call count are explicit positive host inputs.

Each lease buffers at most thirty-two event groups. Final transcript frame
and persistence effect form one indivisible ordered group. Reads retain the
exact numbered batch until acknowledged, reject unknown acknowledgments, and
carry a group to the next batch rather than split it. Replies stay below one
MiB with envelope headroom. Large native audio is split into bounded PCM16
chunks. Unsupported events are ignored; oversized provider text terminates
with a controlled error before it can become an unreadable retained group.

## Host invocation lifecycle

ExecuteLiveTool carries the original admitted provider call ID, exact arguments,
rendering options and saved context token. A separately generated unpredictable
invocation ID lets CancelLiveTool target only that host invocation. Dropping a
native dispatch future sends a cancellation notification with a one-second
deadline. Provider cancellation preserves TinyAgentsLive's established rule:
an already-running tool may finish, but its result is suppressed.

FinishLiveContext revokes the known context after native provider tasks stop.
It also has a one-second deadline; an unavailable/unresponsive host yields
an explicit negative cleanup result, never a fabricated acknowledgment.
Cancel/finish must be idempotent for already completed known work and must
not create or revive unknown contexts/invocations. TinyBus stops waiting
without cancelling remote execution, so the host must cancel its own invocation
leases on endpoint/context teardown as well. Remote notification is bounded
best effort; native provider termination does not depend on receiving it.
