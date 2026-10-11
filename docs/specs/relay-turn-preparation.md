# Relay turn preparation

A host asks the compiled voice module to extract the latest user text and
replayable history from provider-relayed messages. `PrepareRelayTurn` accepts
a JSON message list and a host-selected readback prefix. It returns the
prompt, history pairs, optional readback payload, and content-free, answerable
and speak-back classifications.

Text strings are copied unchanged. Arrays join each string `text` field with
one space, even when a part has no `type`. Other content shapes yield empty
text. History includes only user, assistant and agent roles and drops
whitespace-only messages. The prompt comes from the last user message, even
if its content is empty or malformed.

Readback detection trims leading whitespace before matching the supplied
prefix, then trims the payload. Content-free prompts contain no Unicode
letter or digit. Speak-back is suppressed on readback turns. A prompt is
answerable only when it carries content and is not a readback. Empty prefixes
retain ordinary string-prefix behavior; callers select their own policy.

The caller owns its prefix, credentials, local agent, permissions, storage,
progress projection, delivery and approval context. The bus crate contains
serialized vocabulary only; behavior runs in the compiled module. Hosts
must consume a released module before deleting their implementations. An
unpublished local build provides test evidence, not a replacement release pin.
