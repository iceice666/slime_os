# Work items

`v2/schema.zt` owns the `task/v2` work-item format of the v1.5 devloop, and
`v2/helpers.zt` owns the constructors an item file is written with. An item
is `.tasks/items/<slug>.zt` while open and `.tasks/terminal/<slug>.zt` once
closed. The only hand-written state is `intent`; `done` is derived from the
`evidence` branch.

Until the v1.5 cutover, `task/v2` items exist only in the
`scripts/spec/e2e` fixture: `.tasks/` still holds the V1 MyQue store. At
cutover, `.tasks/items/` and `.tasks/terminal/` receive byte-identical copies
of both files, because Zutai refuses an import above the entry file's
directory. `scripts/spec/tasks check` refuses a copy that differs from this
directory.

`task/v2` is `task/v1` from the living-spec experiment plus three fields:
`intent`, `supersedes`, and the `#acceptedDefault` provenance. `main` never
held a `task/v1` item, so no reader accepts it.

The decision is
[`docs/decisions/work-item-devloop.md`](../../docs/decisions/work-item-devloop.md).
