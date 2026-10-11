# Relay turn preparation implementation

Spec: [relay turn preparation](../specs/relay-turn-preparation.md).

1. Add a real TinyBus regression calling `PrepareRelayTurn`; observe the
   missing member failure before implementing it.
2. Add request/result vocabulary to `tinyvoice-bus` and pure relay preparation
   to `tinyvoice`, with migrated prompt extraction/classification tests.
3. Expose the operation in the module interface, names and ABI manifest;
   advance the additive contract minor and verify dispatch table parity.
4. Test malformed content, Unicode recognition artifacts, custom readback
   prefixes, empty content and role filtering through the module and library.
5. Run formatting, warning-denied Clippy, build, workspace/module tests and
   documentation, then open the canonical upstream PR.
6. Consume a released module in the host separately. Agent execution,
   product configuration and backend transport remain outside this change.

7. Reproduce oversized reply amplification before adding an aggregate request
   limit. Verify exact serialized boundaries, escaped text, aggregate history,
   rejection before copying and same-connection recovery through the real bus.
   Rebuild the native artifact and exercise rejection through its ABI.
