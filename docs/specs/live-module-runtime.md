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
its tasks. Shutdown closes admission, cancels every native task and joins cleanups concurrently under one two-second deadline. Dropping a release/shutdown future aborts the worker rather than detaching its task. Module drop also aborts owned workers. A bound transport coordinator closes its provider on client EOF, input/write failure or cancellation. A host using the manual input/effect API must explicitly release its handle on disconnect. Capacity limits and reservation expiry prevent forgotten reservations from growing the table indefinitely.

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

Contract 1.7 adds PrepareLiveInput, LiveReserve, LiveBindTransport, LiveStart, LiveSend, LiveRead, LiveRelease and LiveShutdown. Reserve precedes provider connection; identical
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

The runtime requires the non-owning connection capability proposed in TinyBus [PR #50](https://github.com/tinyhumansai/tinybus/pull/50). Served module objects store `WeakConnection`; active work upgrades it temporarily. OpenHuman adoption must wait for the prerequisite and a released, admitted TinyVoice artifact.


## Mechanical socket callbacks

Bind a LiveTransport endpoint to a reserved handle before LiveStart. The module then owns client polling, native provider input, event batching, delivery retries and terminal cleanup. A bound lease rejects manual LiveSend/LiveRead to prevent competing readers and duplicate persistence. Identical bindings retry safely; changed ports or binding after start fail.

ReadLiveInput receives the last acknowledged frame sequence. The host retains its next frame until that exact sequence is acknowledged; idle polls return no frame and retain the acknowledgment. The module accepts only contiguous sequences, parses portable controls and ignores malformed later controls as the existing wire protocol does. Socket callbacks return idle promptly; a read exceeding one second closes the module coordinator. Audio and typed text are bounded before native dispatch.

DeliverLiveEffects carries an ordered group and a stable delivery ID (`input-N` or `output-N`) scoped to the random module handle. The host applies effects mechanically and remembers committed identities so a lost reply can retry without repeating persistence. The module retries twice under a one-second deadline per attempt. Typed input persistence precedes native text dispatch; final provider transcript frames precede their persistence projection. A failed delivery stops the coordinator.

Client EOF, callback error, native closure and cancellation stop provider work and invoke CloseLiveTransport under a one-second deadline. This method closes only a saved known transport context; repeated known close succeeds and an unknown context returns false. Module release does not invent a successful cleanup acknowledgment. The host owns context leases and must also cancel independent callback tasks when their endpoint drops; TinyBus call timeout alone does not cancel work already running remotely.

The module retains only a weak reference to its serving connection. Active actors borrow owning handles temporarily; dropping the module aborts its provider and transport workers. Cancellation destructors retain their original runtime handle so dropping a callback future outside a runtime enter context cannot panic.
