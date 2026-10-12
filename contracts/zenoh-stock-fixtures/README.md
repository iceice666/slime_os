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
- the counts (11, 8 and 12) and the bounds a reader enforces before it allocates: a batch is
  at most 512 bytes, a name at most 64.

A record with another `formatVersion`, a group of another size, an odd or non-hexadecimal
`hex`, a batch over the bound, or a field the contract does not name, at the top level or
in any entry and however it is laid out, is refused: the decoder closes the field set
itself, because the derived `decode` ignores a field it does not know. The Rust reader is part of the
implementation, not this planning contract; the scope is
`01a12edd-2cb3-7600-80d9-9069c778e44c` (see the exam in
`scripts/check/check-zenoh-profile0.py`). The decision is
[`docs/decisions/zenoh-stock-peer.md`](../../docs/decisions/zenoh-stock-peer.md).
