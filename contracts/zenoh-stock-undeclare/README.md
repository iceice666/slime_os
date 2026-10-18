# Zenoh stock-peer undeclaration fixtures

`v1/schema.zt` owns the file the undeclaration exam hands from its checker to its Rust tests.
`scripts/check/check-zenoh-profile0.py --slice undeclare` writes `build/zenoh-undeclare/fixtures.zti`
from the captured bytes in `scripts/lib/zenoh_undeclare_exam.py`, validates it against this schema, and
gives its path to the `zenoh_undeclare::` tests in `ZENOH_UNDECLARE_FIXTURES`. The tests read it through
generated bindings, not a hand-written parser.

The record is a container only. The wire is eclipse-zenoh's and the batches are bytes a stock
`eclipse-zenoh==1.0.0` peer sent, so the contract declares no layout for them. It fixes:

- `peer`: the eight batches the peer sent to a Profile 0 connector, in the order it sent them, from its
  INIT_ACK to its CLOSE. The seventh is the undeclaration the exam is about: a DECLARE carrying the
  network QoS extension around an UndeclareSubscriber that names the subscriber by id and nothing else;
- `refuse`: ten variants of that undeclaration the profile must keep refusing, each built from it by one
  named change;
- the counts (8 and 10) and the bounds a reader enforces before it allocates: a batch is at most 512
  bytes, a name at most 64.

A record with another `formatVersion`, a group of another size, an odd or non-hexadecimal `hex`, a batch
over the bound, or a field the contract does not name, at the top level or in any entry and however it is
laid out, is refused: the decoder closes the field set itself, because the derived `decode` ignores a
field it does not know. The Rust reader is part of the implementation, not this planning contract; the
scope is the undeclaration item in `.tasks/items/` named by
[`docs/decisions/zenoh-stock-undeclare.md`](../../docs/decisions/zenoh-stock-undeclare.md).
