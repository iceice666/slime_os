# Zenoh link limits

`v1/schema.zt` owns what a Zenoh Profile 0 link may be declared with. A configuration names a mode, a
batch, the payload, key and attachment sizes, the keys, subscribers and outstanding samples, the stack
the owning component declares, and in heap mode a reassembly timeout and a heap. The contract judges the
declaration before a link is built from it.

It sets no size of its own. Nothing here says how many keys a link may hold or how large a message may
be: the protocol has no such cap, and the repository's `1` and `12` are the demo's single `Counter`. A
configuration declares what it needs and the contract checks the declaration is consistent with the
memory it also declares. The limits it keeps each have a source:

- a batch, which includes its two-byte stream length, must fit one network-service operation, 4096 bytes
  (`DATA_BYTES`, `components/proto/src/network_service.rs`);
- a batch must also hold the largest INIT or OPEN message, 87 bytes plus the two-byte stream length, because
  INIT and OPEN cannot be sent as fragments;
- a key declaration, sent whole in one frame and never fragmented, must fit the batch: the key plus 18 bytes and the
  width of its length field, 19 to 21 with the stream length;
- a key count is at most 65535, because the alias id and the scope every reference carries are 16 bits
  (`ExprId` is `u16` in `zenoh-protocol` 1.0.0);
- a key is at most 65535 bytes, because its length travels as a 16-bit field in the profile, and a
  payload at most 4294967295 bytes, because its length is a 32-bit field;
- a sample count is at most 1000000000, the count past which the contract's own 63-bit arithmetic would overflow;
- the stack must cover the link's arrays, its key and subscriber slots and its queued sample notices.
  That is arithmetic from the code's own arrays, not a measured high-water mark;
- in `fixed` mode the whole message (payload, key, attachment, and the tags and lengths around them), with its frame header and sequence number, must fit one batch less its two-byte stream length and
  there is no reassembly timeout or heap; in `heap` mode the heap must hold every queued sample notice, two per outstanding sample, and the message being reassembled, the
  timeout is positive, and the batch must be larger than a fragment's own framing.

The record is closed: the derived `decode` ignores a field it does not know, so the contract refuses one.
The names `demo`, `large` and `heap` are the three configurations the exam pins; the contract itself
accepts any set of one or more distinctly named configurations that satisfy the rules.

The decision is [`docs/decisions/zenoh-link-limits.md`](../../docs/decisions/zenoh-link-limits.md); the
scope is `01a138d9-368a-703b-98cf-024c9f49e80f`, judged by
`scripts/check/check-zenoh-profile0.py --slice links`. The Rust side is the implementation's, not this
planning contract.
