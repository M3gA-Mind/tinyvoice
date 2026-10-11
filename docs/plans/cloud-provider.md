# Cloud provider implementation order

Specification: [host-supplied cloud voice transport](../specs/cloud-provider.md).

1. Add transport-free request and response DTOs to tinyvoice-bus; use opaque JSON
   for speech so existing response normalization keeps its owner.
2. Add object-safe provider futures and methods; distinguish failure categories
   independently of safe display text.
3. Exercise every operation with a fake provider, round-trip optional metadata,
   and confirm debug formatting conceals connection credentials.
4. Document the composition seam and verify the bus and existing compiled module
   suites. Do not add TinyBus operations or change the native wire version.
5. Consume the contract through the host backend layer after the owner change
   is available, keeping credential resolution and HTTP outside voice selection.

The initial contract and tests were implemented directly under the user's
execution authorization. This document records the reviewable sequence; the
error-category and debug-redaction review follow-ups have RED/GREEN regressions.
