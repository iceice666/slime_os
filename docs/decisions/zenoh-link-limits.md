# Zenoh Profile 0 links declared by a named configuration

**Status:** Proposed
**Related work items:** `01a13597-6866-7abc-9fcf-fc2661a3b578` (this decision's exam and
implementation); Zenoh Profile 0 epic `01a0f5fa-f2c4-71cf-962b-c62cbea2ef68`; the stock-peer item
`01a12f13-a343-7924-af20-18148fc1b0d9` (merged), whose exam this one extends.

## Context

[Zenoh Profile 0](zenoh-profile0-transport.md) was sized for one message, the demo's twelve-byte
`Counter`: a 512-byte batch, a 12-byte payload, a 129-byte key, one key and one subscriber. Those
numbers are the demo contract's (`contracts/rpi5-ros2-demo/v2`), and the code holds them as constants
and as fixed arrays, so a component that wants to exchange anything else cannot.

They are not the protocol's. In `zenoh-protocol` 1.0.0 a subscriber id is an `EntityId` (32 bits), a
key-expression alias an `ExprId` (`u16`), and a message any length: a unicast batch defaults to 65535
bytes, and a message larger than the batch the other side announced is sent as `FRAGMENT` messages. The
protocol crate has no cap on subscribers, declarations or resources.

A stock `eclipse-zenoh==1.0.0` peer (the Python wheel, `mode=peer`, scouting and gossip off, loopback)
was run through a logging TCP proxy, publishing one Put with the demo's 33-byte attachment, with
`transport/link/tx/batch_size` set per run:

| Payload | Batch | What was on the wire |
|---|---|---|
| 5000 | 2048 | three `FRAGMENT`s, 2046, 2046 and 963 bytes; headers `66 66 26` |
| 2200 | 2048 | two, 2046 and 204 |
| 5000 | 512 | ten, nine of 510 and one of 500 |
| 2002 | 2048 | two, 2046 and 6 |
| 2001 | 2048 | no fragment: one `FRAME` |

So the peer fragments exactly when the message does not fit the batch it was told, and a fragment is the
batch less the two-byte stream length. The first fragment carries the start of the serialized
`PUSH`; later ones are raw continuation, so a receiver can decode nothing until the last arrives. The
sequence numbers are consecutive. [`scripts/lib/zenoh_fragment.py`](../../scripts/lib/zenoh_fragment.py)
holds the captures by length, header and sha256, and a splitting rule that reproduces every body.

Profile 0 announces a 512-byte batch and refuses a `FRAGMENT` as `unsupported-message`
(`vectors.txt`: `transport_fragment`, `2601aa`). A link announcing a larger batch would be sent
fragments it cannot take.

## Decision

A link is declared by a **named configuration** in one of two **modes**, and a contract judges the
declaration before a link is built from it.

1. **`fixed` mode.** No heap. Every buffer is an array sized from the configuration, a message must fit
   one batch, and a fragment is refused as `unsupported-message`, exactly as today. `demo` is the
   configuration the first release shipped and does not change. `large` is the same mode with bigger
   arrays.
2. **`heap` mode.** The owning component supplies the storage a fragmented message is reassembled into,
   from its own allocator. A message may be larger than a batch. One message is reassembled at a time,
   a gap in the sequence numbers, a run whose first fragment is not the session's next expected sequence
   number, a whole `FRAME` in the middle, an oversize message and a reassembly that outlives its declared
   timeout are each refused by name, and a storage that cannot hold a message is refused without ending
   the session. A whole run advances the session's expected sequence number by its fragment count, and a
   refused run leaves it where it was.
3. **No allocator in the library.** `components/lib` is `no_std` and has no `alloc`. The runtime's
   `private-heap` is a build feature of a component, and Cargo unifies features across one build, so a
   heap feature on the shared library would switch the heap code on for every component in that build.
   Instead the library takes a small storage trait ("give me this many bytes to reassemble into"), a
   heap-mode component implements it over its own allocator, and a fixed-mode component never sees it.
4. **The contract sets no size.** [`contracts/zenoh-link-limits/v1`](../../contracts/zenoh-link-limits/README.md)
   states what a configuration may declare and checks the declaration is consistent with the memory it
   also declares. The limits it keeps each have a source: a batch plus its two-byte length must fit the
   4096 bytes the network service moves in one operation (`DATA_BYTES`); a key count is at most 65535
   because the alias id is 16 bits, and a key is at most 65535 bytes because its length is a 16-bit field, and a payload is at most 4294967295 bytes because its length is 32 bits; the stack must cover the link's arrays, its key and subscriber slots
   and its queued sample notices; the heap must hold every queued sample notice (two per outstanding sample) and the message being reassembled, because a sample is handed out as a slice of the buffer it was reassembled into. It sets no ceiling on keys,
   subscribers, samples, payload or heap.
5. **The corpus row may go.** Once the decoder reads a `FRAGMENT` the row cannot stay refused, and it
   cannot become an accepted row, because the encoder gate and the cross-check harness read every
   accepted row. The stock-peer exam's corpus judge therefore lets `transport_fragment` leave
   `vectors.txt`, the way `transport_keep_alive` already may, and pins its bytes and class while it
   stays. The other 100 refused vectors keep their name, class and bytes.

The three pinned configurations:

| | `demo` | `large` | `heap` |
|---|---|---|---|
| mode | fixed | fixed | heap |
| batch | 512 | 2048 | 2048 |
| payload | 12 | 1024 | 16384 |
| key / attachment | 129 / 33 | 129 / 33 | 129 / 33 |
| keys / subscribers / samples | 1 / 1 / 4 | 4 / 4 / 4 | 4 / 4 / 4 |
| stack | 16384 | 32768 | 32768 |
| reassembly timeout / heap | 0 / 0 | 0 / 0 | 5000 ms / 149202 |

`large` and `heap` are examples of what the contract admits. Their numbers are mine and no workload backs
them; the exam pins them so the host tests have something exact to build, and changing one means
re-admitting the item.

## Alternatives

- **Raise the constants for everyone.** Every Zenoh component would then hold arrays sized for the
  largest message it might ever see. With no heap that is paid whether or not it is used.
- **Follow upstream's defaults** (a 65535-byte batch, a heap, fragmentation always on). A 64 KiB batch
  array in each of the link's eight stack frames is far past the 16 KiB the demo nodes declare and the 64
  KiB the runtime's worker has, and the network service moves 4096 bytes per operation, so it is a
  different design, not a bigger number.
- **A heap feature on `components/lib`.** Rejected for the feature-unification reason above.
- **Ceilings in the contract** (at most 8 keys, 64 KiB of payload). Rejected: they have no source, the
  exam would pin them, and changing one after review re-admits the item.
- **Fragmentation in fixed mode.** It needs a reassembly buffer, which is the thing fixed mode does not
  have.

## Consequences

- A component chooses a mode by what it passes: arrays, or storage from its allocator.
- The stack rule is arithmetic from the code's own arrays (a link's locals are several batch-sized arrays
  and a scratch twice that size), not a measured high-water mark. The implementation's host tests hold it
  to `size_of` of the real types, and a heap link's notices are assumed to hand out a slice of the
  reassembly buffer instead of copying the payload, which the implementation must do.
- The stock-peer exam's three-fold pin on `transport_fragment` becomes a pin that allows it to go.
- Fragments are accepted only in the reliable form. Best-effort fragments, fragments with extensions and
  interleaved messages stay refused.

## Limits

- One peer, one wheel, one machine, one run each. The fragments replay deterministically; the peer's
  behaviour was not measured beyond what is stated here.
- Only reliable fragments of one `Put` with the 33-byte attachment were captured, at batch sizes of 512
  and 2048.
- The edge between a message that travels whole and one that is fragmented was observed at two payload
  sizes (2001 and 2002 bytes at a 2048-byte batch) and is otherwise the rule in the host reference.
- What a stock peer does after we refuse an oversize message was not tested.
- The exchange stays on the host: no QEMU, no NIC. Whether the network service's one-operation limit
  holds with fragments in flight is a claim about the guest and is not observed.

## Revisit when

- a real component declares a message that these configurations do not fit;
- the stack rule is measured, which may loosen the factor of eight;
- the optional attachment or a fabric-based byte link is taken up, both of which are non-goals here.

## References

- [`zenoh-profile0-transport.md`](zenoh-profile0-transport.md), [`zenoh-stock-peer.md`](zenoh-stock-peer.md)
- [`../../contracts/zenoh-link-limits/README.md`](../../contracts/zenoh-link-limits/README.md)
- [`../../scripts/lib/zenoh_fragment.py`](../../scripts/lib/zenoh_fragment.py),
  [`../../scripts/lib/zenoh_links_exam.py`](../../scripts/lib/zenoh_links_exam.py)
- [`../architecture/zenoh-transport.md`](../architecture/zenoh-transport.md)
