# Zenoh Profile 0 taking a stock peer's undeclaration

**Status:** Proposed
**Related work items:** `01a1514d-8af5-7fa1-a4c2-70be8785fb2e` (this decision's exam and
implementation); `01a1514d-caa7-74ff-8fca-c4fcda87aba1` (the external-listener exam, which
admits the form in its capture judge and replaces `01a13948-fc94-7359-ae7d-31c029147cf7`); the
stock-peer session work `01a12f13-a343-7924-af20-18148fc1b0d9`; Zenoh Profile 0 epic
`01a0f5fa-f2c4-71cf-962b-c62cbea2ef68`. Both items were admitted twice in review: the first pair
(`01a14f09-f9a8-7f9d-9c92-543d54a7712e`, `01a14f0b-2bbc-7d80-9ecd-e6bac02970e5`) pinned 62 controls
and is cancelled and superseded, as the QEMU gate item was in #268; neither was started.

## Context

The stock-peer exam ([Zenoh Profile 0 and a stock Zenoh peer](zenoh-stock-peer.md)) replayed
what an `eclipse-zenoh==1.0.0` peer sent while connecting and while listening. In those runs the
peer never ended its subscription, so no capture held an `UndeclareSubscriber` from it. The exam
built its undeclaration refusals from the accepted corpus row `frame_undeclare_subscriber_sn7`,
which is the encoding the corpus was written with: header `0x83`, the subscriber id, and a
wire-expression extension that restates the key.

The external-listener exam ([Zenoh Profile 0 guest connecting out to a stock
listener](zenoh-external-listener.md)) is the first run where the stock peer subscribes, receives
the guest's four samples and then closes. The listener's session closes once all four have
arrived, and before its `CLOSE` it undeclares its subscriber. The guest's session refused that
batch and the connector reported a link failure after the samples were already on the wire:

```text
25 fe cc bf 5a   9e 21 08   03 01
FRAME sn          DECLARE    UndeclareSubscriber, id 1
                  + QoS 8    header 0x03: no Z flag, no extension
```

The same session also sent seven batches the stock-peer exam already covers: `INIT_ACK`,
`OPEN_ACK`, an `OAM`, the `DeclareKeyExpr`, the `DeclareSubscriber` with its `DeclareFinal`, a
`KEEP_ALIVE` and the `CLOSE`.

## What was established

The undeclaration names the subscriber by id and nothing else. The upstream 1.0.0 codec writes the
`UndeclareSubscriber` header with the `Z` flag and the wire-expression extension only when the
expression is not null (`commons/zenoh-codec/src/network/declare.rs`, the `WCodec` for
`UndeclareSubscriber`). A subscriber declared by alias has a null expression to undeclare, so the
bare form is the encoder's ordinary output, not a quirk of the Python binding. The router removes a
subscription with `remote_subs.remove(&id)` and does not read the expression
(`zenoh/src/net/routing/hat/p2p_peer/pubsub.rs`, `forget_simple_subscription`), so the message is
complete as sent.

Profile 0 refuses it for a reason of its own. Its decoder reads an undeclaration as header `0x83`
with the wire-expression extension, and the stock-peer decision refuses the QoS extension on an
`UndeclareSubscriber` because no capture had one (`refuse-qos-on-undeclare-subscriber`). The two
nearby refusals are different bytes with different classes, and both are pinned:

| Batch | Class today | Pinned by |
| --- | --- | --- |
| `25 01 1e 03 01`, the bare form without the QoS | `malformed-message` | the vector `usub_missing_wire_expr_ext` |
| `9e 21 08` + the full form `83 01 5f ...` | `unsupported-extension` | `refuse-qos-on-undeclare-subscriber` |
| `9e 21 08 03 01`, what the peer sent | `unsupported-extension` | nothing: no capture held it |

## Decision

Profile 0 takes one more shape from a stock peer, and nothing wider.

1. **The shape.** A `DECLARE` that carries the network QoS extension (`21` with a value of at most
   31) around an `UndeclareSubscriber` whose header is exactly `0x03`, followed by a canonical
   32-bit subscriber id and nothing else. The QoS is the `DECLARE`'s own, in the position the
   stock-peer decision already accepts for a `DeclareKeyExpr` and a `DeclareSubscriber`.
2. **The session rule is "by id alone".** The session removes the remote subscriber it holds under
   that id and reports it undeclared. An id it does not hold, and a repeat, are `unknown-subscriber`
   and leave the session as it was. The form carries no key, so matching on one is impossible, and
   the full form's match on id and key is not weakened: the full form keeps its own arm.
3. **What stays refused,** each by name and class in the exam: the bare form without the QoS
   (`malformed-message`, the pinned vector), the full form behind the QoS (`unsupported-extension`,
   the pinned refusal), a reserved header bit 5 or 6 (`malformed-message`), no id (`truncated`), an
   id over 32 bits (`leb128-overflow`) or not canonical (`leb128-noncanonical`), and a trailing
   byte (`unsupported-message`). The classes were measured on the decoder with the change applied,
   not assumed.
4. **Where the exam lives.** A new slice, `check-zenoh-profile0.py --slice undeclare`, as
   `--slice links` was added, with its expectation in `scripts/lib/zenoh_undeclare_exam.py` and its
   hand-off in the new contract `contracts/zenoh-stock-undeclare/v1`. The host reference
   `scripts/lib/zenoh_wire.py` gains `undeclare_subscriber_by_id`, which reproduces the captured
   batch byte for byte. The slice runs the stock corpus judge, the retained suites and the
   stock-peer exam's tests beside it.
5. **The stock-peer and links items do not move.** No row of `vectors.txt` changes, no accepted row
   is added, and `zenoh_stock_peer.py`, the stock fixtures contract and the links exam are not
   edited. The stock gate's 69 cases and 138 controls and the links gate's 286 cases and 669
   controls are the same numbers, and both items are unstarted with no evidence, so the one edit
   they share, to `zenoh_wire.py`, stales nothing.
6. **The external-listener exam admits the form in its capture judge,** and only in that one shape.
   Its judge accepted the full form only; it now also accepts the bare form behind the QoS and
   holds the same facts: the subscriber was declared, is still held, and is named by an id it
   holds. That changes its counts (below), and a stored body is bound to a digest, so the item is
   replaced as `#268` replaced the QEMU gate item: cancelled, with a successor admitted under a
   new token, that depends on this item. It was unstarted with no evidence.

## Alternatives considered

- **Fold it into the stock-peer item.** The capture belongs with the other stock-peer batches. It
  moves the case and control counts of two landed items, edits `zenoh_stock_peer.py` and the
  fixtures contract, and replaces both items. Rejected for that cost; the new item is smaller and
  nothing else moves.
- **Match by id and the held key.** A key-less message cannot carry a key, so this reduces to the
  chosen rule.
- **Accept the bare form without the QoS as well.** Upstream's encoder would send it when no QoS is
  set. The vector `usub_missing_wire_expr_ext` pins it as refused and the stock-peer item holds
  that vector, so changing it is a re-admission of that item for a shape no capture shows.
- **A second reference in the new module.** It would put the wire in two places.

## Consequences

- `just zenoh_undeclare_check` fails until the decoder and session take the form, and names the
  Rust test that did not pass.
- The exam pins 20 cases and 68 corrupted records (44 transcripts, 18 hand-off records and 6
  corrupted inputs to the check that the host reference reproduces the capture),
  exactly, and compares them with the admitted item while it is active.
- The external-listener exam pins 4 cases and 291 corrupted records (246 + 15 + 30), where it
  pinned 279 (234 + 15 + 30), and 46 honest captures where it pinned 45. The 12 added mutations
  are the ones that follow from the new shape: by id before the first sample, an id not held, twice,
  by id and then in full, in one frame with its declaration, without the QoS, with the extension
  flag and no extension, with a reserved bit, with no id, with a trailing byte, with a QoS value over
  the bound, and the full form behind the QoS. The added honest capture is the stock peer
  undeclaring by id after the last sample and then closing.
- Adding a recipe to `just/hardware.just` changes a closure input, so the system-image closures
  and test-run records are regenerated in their own commit.
- The implementation changes `Network` in `zenoh_profile0.rs`, which adds a match arm to four
  existing tests (`tests.rs`, `encode_tests.rs`, `zenoh_stock.rs`) that match on it exhaustively.
  Those arms add no captured byte and move no pinned number.

## Limits

- One capture of one peer: `eclipse-zenoh==1.0.0` on x86_64 Linux, in peer mode with scouting and
  gossip off, over loopback TCP through QEMU user networking. Another QoS value, another peer, a
  router and `rmw_zenoh` are not observed.
- The bare form without the QoS stays refused, though the upstream encoder would send it for a
  message with no QoS. No capture shows it, and the pinned vector refuses it.
- The session rule matches by id alone. A peer that declared a subscriber under an id and
  undeclares a different key under the same id cannot be told from a correct undeclaration, as it
  cannot at the upstream router.
- The exam proves the session takes the batch. It does not prove the guest completes the external
  exam: that is the successor item's recipe, run after the implementation.
