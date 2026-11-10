# Observation records

`v1/schema.zt` owns the three record kinds the `evidence` branch holds: a
machine `Record` per target and closure digest, a signed `HumanRecord` per
human acceptance, and a signed done `Witness` per work item. No record is
ever committed to `main`.

`scripts/spec/record` writes machine records, `scripts/spec/human` writes
human records, and `scripts/spec/ci --witness` writes witnesses.
`scripts/spec/publish` carries all three to the `evidence` branch. Every
reader type-checks a record against this file before it trusts it.

The decision is
[`docs/decisions/work-item-devloop.md`](../../docs/decisions/work-item-devloop.md).
