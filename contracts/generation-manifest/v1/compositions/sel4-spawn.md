# `sel4-spawn.zti` — supervised spawn composition

The [`sel4-spawn.zti`](sel4-spawn.zti) manifest owns the three-instance graph,
its grants and slots, and `bootAction = "spawn"`. The
[spawn scenario](../../../../components/lib/src/spawn_plane.rs) resolves declared
executable names at runtime; scenario selection is authenticated generation data,
not a compile-time environment switch.

## Authority and slots

Init has `spawnBudget = 4`. Its executable grants name `console` at slot 1 and
`sysinfo` at slot 2. The children are `autostart = false`: init constructs them
from those grants rather than accepting duplicate root-launched instances.
`console-control` and `sysinfo-context` are ordinary generation-declared native
Endpoints; the root installs the declared ends, not a parent-supplied spawn grant.

What init supplies at spawn is transferable directory authority: one read-only
view for console and six for sysinfo. The source slots are 5 and 6–11; the child
slots are 1 and 1–6 respectively. Six grant records occupy 96 bytes, so the
scenario exercises the staged grant array beyond the 64-byte message bound.
The manifest declares no minted bindings or shared-buffer budget.

[Spawn admission](../../../../slime-root/src/graph_runtime/services/spawn.rs)
checks the requested rights against both the child's declaration and the parent's
held capability, rejects duplicate sources and use of the executable as a child
grant, and installs narrowed copies. The parent's directory views remain usable.
The executable's transfer right controls whether the returned supervision handle
is transferable; these two executable grants are not transferable.

## Verification boundary

The [spawn checker](../../../../scripts/check/check-sel4-spawn-plane.py) requires
ungranted and widened spawn denials, child construction with one and six grants,
retention of the parent's views, a still-live console outcome, and clean sysinfo
collection. These are acceptance requirements, not a fresh execution record.
