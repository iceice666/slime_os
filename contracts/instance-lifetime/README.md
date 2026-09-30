# Instance lifetime, version 1

`v1/schema.zt` defines the generation resource that names the instances a
composition declares resident. Generate its Python layout and Rust constants with
`python3 scripts/generate/generate-boot-bindings.py`.
`boot-contracts/src/instance_lifetime.rs` owns decoding.

## Meaning

- **resident**: the instance runs for the life of its graph. The root treats
  its exit as a failure, never as completion.
- **bounded**: the instance may finish its work and exit.

A composition declares the lifetime per instance. The component spec's
`lifetime` is the default, and a system-spec placement or instance record may
override it. Every instance the object does not name is bounded. The builder
emits the object only when some instance is resident, so a generation without
it has every instance bounded.

## Encoding

The 32-byte header contains magic `SLIMELT\0`, format version 1, header size,
zero required flags, row count, and exact total length. At most 48 rows follow,
one per resident instance. Each row is 36 bytes:

| Field | Width | Meaning |
| --- | --- | --- |
| `instance_identity` | 32 | SHA-256 over `slime-instance-lifetime-v1`, the name length as `u16` LE, and the instance name |
| `lifetime` | 1 | Always `1` (resident); a bounded instance is expressed by its absence |
| `reserved` | 3 | Zero |

Rows are sorted strictly by identity. The decoder refuses a zero or duplicate
identity, unsorted rows, any lifetime other than resident, nonzero reserved
bytes, unknown required flags, and trailing bytes. The root refuses at admission
a row naming no admitted instance.

## Enforcement

- A resident required instance that exits, with any status, ends the graph with
  `SLIME_GRAPH FAIL resident instance <name> exit status=<status>`.
- A resident optional instance that exits is recorded as
  `SLIME_GRAPH resident exit instance=<name> status=<status> recorded=unhealthy`.
  Its supervisor's `SUPERVISION STATUS` answers unhealthy (`4`), not exit.
- A resident instance never counts as completed. A graph holding a live resident
  required instance certifies with it counted live once every bounded required
  instance has completed.

A component reads its own lifetime with `LIFECYCLE LIFETIME READ` (label 73),
which `slime_rt::lifetime()` wraps. See `docs/decisions/instance-lifetime.md`.
