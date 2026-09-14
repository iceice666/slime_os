# Decisions

This directory owns important long-lived cross-module choices whose rationale
must survive any one implementation or review system. It is not a tracker and
not a per-PR diary; canonical work identity, state, hierarchy, dependencies, and
observed exit conditions remain in `.tasks/items/`.

Each decision record states:

- status: `proposed`, `accepted`, or `superseded`;
- context and the decision;
- alternatives and trade-offs;
- consequences and revisit conditions;
- relevant canonical work-item UUIDs and code or contract references.

Moving an old proposal here does not make it accepted. Superseded records remain
readable and link to the decision that replaced them.

## Records

- [Development record ownership](development-record-ownership.md) — accepted;
  separates current knowledge, work state, change evidence, durable rationale,
  and retained history during the repository split.
