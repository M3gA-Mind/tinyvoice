# Compiled relay turn coordination

The compiled voice module owns generic execution-slot scheduling, selected
progress polling, foreground acknowledgment and deferred completion. The host
retains its local brain, model/thread selection, progress eligibility,
correlation admission, spoken phrases, failure-notice eligibility and product
destinations. The module never obtains a credential or fetches a task source.

Configure explicit concurrency and reservation limits with RelayConfigure.
Configuration is idempotent for identical limits and fixed for the module
instance. Active turns range from one to sixteen; pending/retained leases
range from the active limit to 256. RelayShutdown permanently closes
admission. Reserved leases expire after sixty seconds without evicting
active turns.

RelayReserve returns an unpredictable opaque handle. Before RelayStart, the
host binds that handle to an existing saved turn context. Every RelayHost
callback verifies the context/handle pair, including effects emitted while
queued and cleanup before execution starts. Request fields never manufacture
authority. A declaration or reservation never grants a native executor.

RelayStart accepts the callback endpoint, an acknowledgment deadline, an
overall timeout and explicit empty/deadline/lost spoken fallback text.
Identical retries retain the existing task; changed requests fail. Startup
JSON is at most 64 KiB. The acknowledgment deadline is positive and at most
sixty seconds; the overall timeout includes queue/admission time, must cover
the acknowledgment deadline and is at most thirty minutes.

## Host callbacks

The fixed interface is ai.tinyhumans.tinyvoice.RelayHost.

| Method | Responsibility |
| --- | --- |
| StartRelayBrain | Start the saved native brain once, returning false for unknown or currently revoked authority. Repeating a known invocation never creates another brain. |
| ReadRelayBrain | Return an idle poll or the retained next selected delta/completion. Acknowledge only the last completely applied event. Native completion follows all preceding selected deltas. |
| ApplyRelayEffects | Apply one ordered group and remember its delivery ID within the known invocation. Lost replies can retry without repeating committed effects. |
| FinishRelayBrain | Stop only the known invocation and its owned work; repeated known cleanup succeeds, unknown cleanup returns false and never revives work. |

A callback waits at most one second. Idle reads return promptly rather than
waiting for the whole native turn. Remote TinyBus timeout does not cancel work
already running on the host: the host owns invocation leases and must stop
them on finish and endpoint teardown. Finish reports true only when its known
work has stopped. Do not synchronously call RelayRelease from a finish
callback that the coordinator is awaiting; release after replying or schedule
mechanical cleanup without blocking the reply.

The acknowledgment clock starts before waiting for an execution slot and
stays live while native admission is pending. A slow turn emits the explicit
host acknowledgment only if no selected text has streamed, then emits done.
Native work continues within the overall deadline. Late selected deltas
retain their existing ordering; a late completion emits a deferred outcome
for the host to apply its own chat/readback policy.

A timely success emits the trimmed whole reply only if no selected delta was
streamed; empty replies use the explicit host fallback. Otherwise selected
deltas precede done without a duplicate whole reply. Whitespace deltas remain
significant; truly empty deltas are skipped. A reported native failure emits
an error. Lost native work emits a loss fact and the supplied fallback/done
when foreground is still open. A coordination failure reports bounded
generic error/loss facts without inventing a native result or successful
execution.

Progress JSON and effect groups are at most one MiB; individual selected
deltas and completion text are at most 256 KiB. Apply retries at most twice
with the same delivery ID and within the remaining turn budget. Exhausted
acknowledgments are indeterminate: no new contradictory failure effects are
sent after a possibly committed group. These bounds refuse new/unacknowledged work rather
than pretending it committed.

## Lifecycle and verification

The execution slot remains owned until native finish completes. If native
cleanup cannot be confirmed, execution-slot admission closes instead of
starting more work while the earlier brain may still be running. Final
overall-timeout notifications follow native cleanup under their separate
bounded callback budget; the host keeps the registered context available for
these terminal effects after finish stops the native work.

Release owns its worker join under a two-second deadline. Dropping a release
future aborts the worker rather than detaching it. Shutdown joins cleanups
concurrently; module drop aborts owned coordinators and sends bounded
best-effort finish notifications through the original runtime handle. Cleanup
failure remains observable instead of becoming an invented success.

Native deterministic fixtures cover acknowledgment before a slot is
available, acknowledgment while admission is pending, selected delta order,
deferred completion, current authority, reservation bounds and cancellation.
The real loaded-module example runs a native host fixture task, verifies
ordered effects and lost-reply deduplication, and proves a revoked context
starts no native task.

This is separate from the live provider/socket runtime in
[live-module-runtime.md](live-module-runtime.md). Source prerequisites must
land and an actual released, admitted module must exist before OpenHuman
adopts either runtime; no host release pin changes here.
