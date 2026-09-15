# `sel4-supervision.zti` — supervision lifetime composition

The [`sel4-supervision.zti`](sel4-supervision.zti) manifest owns
`bootAction = "supervision"`, init's two executable grants, and the endpoint-free
`supervision-child` declaration. The
[current scenario](../../../../components/lib/src/supervision_plane.rs) exercises
handle lifetime across repeated child construction and collection.

## Authority and lifetime

Init resolves `supervision-child` by executable name; its declared slot is 1,
with `exec`, `spawn`, and transfer authority. `sysinfo` is separately declared at
slot 2 without transfer authority, but is not the loop subject. There are no
endpoint grants, minted bindings, or shared-buffer quotas in this composition.

`spawnBudget = 4` bounds live children, not lifetime constructions:
[spawn admission](../../../../slime-root/src/graph_runtime/services/spawn.rs)
uses `TaskTable::live_children`. The executable's transfer bit controls the
transferability of the returned supervision capability; it does not grant a
child arbitrary authority.

The scenario derives a second handle, collects the original, then runs 49
spawn/collect iterations. The derived handle must still report the original
child's clean exit after the loop and must be refused after its own collection.
The child requires no endpoint, so this exercises supervision lifetime without
allocating an unrelated channel for each iteration. This scenario uses a derived
handle, not an exported capability parked on a self-loop endpoint.

## Verification boundary

The [supervision checker](../../../../scripts/check/check-sel4-supervision-plane.py)
compares the configured loop against the root's record bound and requires the
lifetime-crossing, derived-handle survival, consumed-handle refusal, and no
outstanding export markers. Those requirements do not constitute fresh runtime
results or evidence for an in-transit export scenario.
