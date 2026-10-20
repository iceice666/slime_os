# Zenoh service grants

`v1/schema.zt` owns what the Zenoh service may do for each holder. The service owns one Zenoh link and serves many local components. A record holds the service's limits and a list of rows. A row names a holder, one exact key, and the rights to publish and to subscribe. A key string in a request grants nothing. Only a row in an admitted record grants a right.

The contract sets no count of its own. The record declares the holders, the delivery slots per holder, the sample bytes, the key bytes, the local subscriptions and the stack. The contract checks the declaration against that stack.

Each remaining bound has a source.

- A key is at most 65535 bytes, because its length travels as a 16-bit field in the profile ([`zenoh-link-limits/v1`](../zenoh-link-limits/README.md)).
- A count is at most 65535 and a sample at most 1048576 bytes. The largest product the contract forms then stays under the 63-bit integers of its arithmetic.
- A holder is named as a binding is in `network-application/v3`. It uses 1 to 32 lowercase letters, digits, hyphens and underscores.
- A key uses ASCII letters, digits and the characters `/ _ - : . ~`. It has no empty segment. The contract refuses a wildcard, a query mark, a fragment mark and a non-ASCII character. The demo's key and an `rmw_zenoh` key (`<domain>/<topic>/<type>/<hash>`) are inside that set.
- The rows may not name more holders than the record declares.
- The service declares one subscription to its peer for each distinct subscribe key. Those keys may not exceed the local subscriptions that the record declares.
- The stack must cover the service's fixed storage, a header per holder, every holder's ring, and a header and key per row. The schema states each overhead as a constant. Each constant is an upper bound on a `size_of`. The service core exam prints those sizes and refuses one above its constant.

The record is closed. The derived `decode` ignores an unknown field. The contract therefore refuses one at the top level, in the limits and in every row.

The decision is [`docs/decisions/zenoh-service.md`](../../docs/decisions/zenoh-service.md). The scope is `01a159c2-026a-7358-b6de-102b2d8e0d44`. The checker is `scripts/check/check-zenoh-profile0.py --slice grants`. The implementation owns the Rust bindings and the path of a record from the system spec to the service.
