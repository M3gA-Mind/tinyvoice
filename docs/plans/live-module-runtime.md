# Live module runtime implementation

Specification: ../specs/live-module-runtime.md.

1. Add an actual bus missing-operation regression for live frame preparation.
   Move portable wire vocabulary and event projection into the bus/library,
   preserving serialized golden frames and persistence ordering.
2. Consume TinyAgentsLive from its canonical recorded source as a module
   implementation dependency. Add a callback tool adapter with explicit host
   context and current-policy enforcement at the host endpoint.
3. Add bounded random session handles and duplex reserve/start/send/read/release/shutdown
   operations. Replay event batches until acknowledged and retain terminal
   events until explicit release.
4. Verify a real compiled native module against a local provider and a
   policy-aware callback; migrate websocket and lifecycle fixtures.
5. Add the portable relay coordinator with host timing/completion ports where
   extraction preserves the existing acknowledgment and deferred delivery
   semantics. Keep product model, progress filtering and delivery choices out
   of the module.
6. Run owner workspace/module formatting, strict Clippy, tests, builds,
   documentation, native artifact verification and per-file coverage.
   Publish the owner prerequisite separately from released host adoption.
