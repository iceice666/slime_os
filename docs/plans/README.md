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

## Extracted requirements

- [Memory capacity](memory-capacity.md) — target-bound multi-holder capacity
  and reclamation qualification beyond the existing memory mechanism.
- [Network data plane](network-data-plane.md) — real TCP bytes over LinkDevice,
  exact destination authority, and bounded reset/restart behavior.
- [Framework hardware](framework-hardware.md) — device qualification after
  CPU boot, IOMMU containment, recovery, and the internal-storage safety boundary.
- [Authority and trust](authority-and-trust.md) — revocation, secrets,
  accelerator authority, attestation, and distributed capabilities.
- [RPi5 ROS 2 demo](rpi5-ros2-demo.md) — bounded middleware interoperability
  and the exact physical-board evidence boundary.

The [roadmap classification](../../roadmap/README.md) names every retained
source and any detailed requirements not yet extracted. Those explicitly
retained sections remain the owner of that detail; extracted subjects use the
pages above. Historical investigations are preserved in the [archive](../history.md).
