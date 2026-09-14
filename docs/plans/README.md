# Plans

This directory owns unimplemented designs, delivery requirements, and
qualification requirements that are specific enough to guide future work but do
not describe current behavior.

Plans are not a tracker. Identity, state, priority, hierarchy, dependencies, and
observed exit conditions live only in `.tasks/items/`. A plan links canonical
work-item UUIDs when work exists; editing a plan never creates, starts, closes,
or reorders an item.

Keep current architecture, operation, and limitations in the owning product or
contract documentation. Keep important accepted or proposed cross-module choices
in `docs/decisions/`, with explicit status and revisit conditions. Keep
exploratory possibilities that are not committed work in `docs/directions/`.
Delivery chronology and historical verification results do not belong in a plan.

`roadmap/` currently contains a mixture of these categories. H2 will classify it
subsystem by subsystem; until then, do not copy its contents here or create a
second competing specification.
