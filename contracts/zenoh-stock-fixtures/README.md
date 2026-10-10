# Zenoh stock-peer fixtures

`v1/schema.zt` owns the file the stock-peer exam hands from its checker to its Rust
tests. `scripts/check/check-zenoh-profile0.py --slice stock` writes
`build/zenoh-stock/fixtures.zti` from the captured bytes in
`scripts/lib/zenoh_stock_peer.py`, validates it against this schema, and gives its path to
the `zenoh_stock::` tests in `ZENOH_STOCK_FIXTURES`. The tests read it through generated
bindings, not a hand-written parser.

The record is a container only. The wire is eclipse-zenoh's and the batches are bytes a
stock `eclipse-zenoh==1.0.0` peer sent, so the contract declares no layout for them. It
fixes:

- `cookieHex`: the 29-byte cookie our side issued, which the stock peer echoed in its
  `OPEN_SYN`; a replay of the `connects` group configures its listener with it;
- `connects`, `listens`, `refuse`: ordered lists of `{ name; hex }`. The order is the
  order the bytes were captured and is part of the exam, so a reader must replay a group
  in list order;
- the counts (11, 8 and 5) and the bounds a reader enforces before it allocates: a batch is
  at most 512 bytes, a name at most 64.

A record with another `formatVersion`, a group of another size, an odd or non-hexadecimal
`hex`, or a batch over the bound is refused. The Rust reader is part of the
implementation, not this planning contract; the scope is
`01a125c9-b193-7ab7-8ea0-43bbac09e946` (see the exam in
`scripts/check/check-zenoh-profile0.py`). The decision is
[`docs/decisions/zenoh-stock-peer.md`](../../docs/decisions/zenoh-stock-peer.md).
