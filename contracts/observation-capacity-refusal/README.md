# Observation capacity refusal

`v1/schema.zt` owns an immutable local replay record for a recipe that executed
but exceeded the observation gate's 65,536-byte raw stderr buffer. It binds the
gate kind, complete execution identity, recipe, raw count/bound, recipe exit and
the original diagnostic receipt digest/path. It is not a failed recipe
observation and never permits successful acceptance.

The operator discovery line is:

```text
SLIME_DEVLOOP_CAPACITY receipt=<checkout-relative-path> run=<run-key>
```

The ordinary diagnostic discovery line remains required. A repeated identity,
recipe and gate within `[finishedAt, reuseUntil]` replays the refusal without
executing the recipe, replacing the original records or changing historical
evidence. `reuseUntil` is exactly `finishedAt + reuseSeconds`. The stored record
is immutable; tests age a fixture record or clock only in isolated stores.

The record is refused unless its schema, version/kind/outcome, gate, complete
identity, key, raw byte count and diagnostic binding agree with the execution.
Paths are checkout-relative and bounded; symlinks and escaping paths are refused.
Receipt bytes are bounded by the schema. A refused recipe may have already
performed side effects; this is post-execution capacity classification, not
admission-time protection or a sandbox.

Digest oracle below the bound: decode each stream independently as UTF-8 with
`surrogateescape`, translate CRLF and lone CR to LF, then re-encode with UTF-8
`surrogateescape`, preserving state across chunks. SHA-256 hashes all normalized
stdout followed by all normalized stderr. Retained raw logs are byte-exact and
use no newline normalization. Invalid UTF-8 support is new; it does not claim a
digest from the formerly strict decoder.

The scope is `01a11ff0-9c8d-714f-a374-fb8e641eb873`. Its executable exam must land
before implementing this replay format. Generated codecs belong in the later
implementation, not this planning contract.
