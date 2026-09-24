# Adaptive memory guarantee reservation

- Status: **accepted**
- Work item: [01a0bd36-7f01-76cd-873d-b9c03687fd9f](../../.tasks/items/01a0bd36-7f01-76cd-873d-b9c03687fd9f.md)
- Parent identity: `01a09b65-4386-7b79-a64a-f263de037b8d`

## Context

An adaptive private-memory guarantee is a simultaneous promise, not permission
for a holder to compete for whatever memory remains. Elastic maxima remain
permissions: admission must not reserve their sum. A cohort entitlement shares
one guarantee across its subjects; it does not acquire a separate guarantee for
each task incarnation.

The current resource ledger and allocator are separate mechanisms:

- [Generation admission](../../slime-root/src/generation.rs) constructs resource
  guarantees and admits them against a supplied inventory.
- The [policy ledger](../../boot-contracts/src/private_memory_policy/ledger.rs)
  tracks entitlement, incarnation, pending transaction, refund, and quarantine.
  Its placement validator accepts ordinary free sources, not protected
  `Guaranteed` sources.
- [Elastic allocation](../../slime-root/src/object_allocator/elastic.rs) acquires
  actual extents, records their placements, and manages rollback. Inventory
  includes potentially fundable metadata capacity; the descriptor and extent
  estimates draw from the same metadata window.
- [Task construction](../../slime-root/src/task.rs) consumes static backing and
  capabilities before activation. [Graph launch](../../slime-root/src/graph_runtime.rs)
  stages root-autostart tasks before activating them.
- [Growth planning](../../slime-root/src/private_memory/elastic.rs) selects large
  frames for suitable fresh spans. The
  [mapping implementation](../../slime-root/src/private_memory.rs) must agree
  with that shape.

These mechanisms do not, by themselves, establish a physically isolated
entitlement reservation. Subtracting a tuple in the ledger does not stop other
allocator consumers spending its backing. A numerical free-byte floor also does
not preserve an aligned large-frame placement. Successful qualification of
individual mechanisms is not evidence that their proposed integration below is
implemented or qualified.

## Proposed decision

Use **physically protected, per-entitlement reservation pools** for guarantees,
with a **base-page guaranteed mapping path**. Ordinary elastic allocation and
fixed-policy allocation retain their existing large-frame behavior. No adaptive
activation may bypass this reservation boundary merely because its resource
ledger admits a numerical inventory.

### Physical ownership and complete resource funding

A reservation has an allocator-owned identity independent of task incarnations.
Its backing consists of actual aligned untyped sources removed from ordinary
allocation. Every borrowed extent records its reservation origin. Common
inactive-extent reuse must exclude reserved sources, including after failed
construction and task teardown.

Admission must fund all of the following simultaneously:

1. Guaranteed payload backing and required leaf-table backing.
2. Object CSlots and allocation records for that backing.
3. Reservation parent and child anchors, extent records, and any split/merge
   bookkeeping.
4. The metadata pages and capability hierarchy needed to hold those resources.

Actual metadata must be funded jointly before the final inventory is judged.
Two capacity estimates cannot each promise the entire remaining metadata window.
Reserved record positions and slots must be unavailable to ordinary consumers;
an unmaterialized capacity estimate is not a reservation.

A simple conservative implementation can prepartition protected backing into
independently revocable granule leaves. That may be expensive in capabilities and
records, but its complete cost can be admitted explicitly. Lazy subdivision is
an alternative only with a checked structural resource bound and rollback
ownership. The [shared backing implementation](../../slime-root/src/object_allocator/shared_backing.rs)
is a reference for buddy ownership, not proof that its bounds apply unchanged.

Do not assign an entire coarse parent to the first cohort member that requests a
page. That would strand unused backing from other members. The parent remains
reservation-owned; borrowed children are independently revocable. A parent with
live descendants belonging to other tasks cannot be revoked to reclaim one task.

### Guarantee mapping and table retention

Guaranteed payload uses base-page mappings, so its promise does not depend on a
future 2 MiB placement. Elastic payload can still select large frames. Planner,
acquisition, and mapper must consume the same explicit mapping plan. Changing
only accounting while the mapper independently chooses large frames is invalid.

For `g` guaranteed payload pages, `g` leaf-table pages is a conservative initial
bound across arbitrary subjects and windows. A bound of `ceil(g / 512)` is not
sufficient for a cohort distributed across many address spaces.

**The table bound has an unresolved implementation and proof obligation.** It
is valid over retries only if each live guarantee-funded table retains at least
one charged guaranteed payload-page claim, or a quarantined guaranteed-page
claim, until the table is reclaimed. Empty tables retained by repeated failed
growth must not accumulate while those claims are refunded.

The proposed resolution is to record newly installed guaranteed tables in the
transaction receipt and remove newly created empty tables before successful
rollback refunds the associated guarantee. Failed unmap/revoke quarantines the
backing and page claim. Each table needs a distinct charged page claim. Cleanup
orders table unmap, child revoke, return to its reservation, and only then refund;
a failure before refund retains ownership. Pre-existing tables remain owned and charged to their
existing owner. Incarnation retirement refunds only after successful reclamation.

The ledger currently allows successful rollback to retain only elastic-funded
table resources. Preserve that boundary physically: relabeling reservation-owned
backing as elastic is not funding. Retaining such a table requires an explicit,
fully funded ownership transfer, or must remain a quarantined guaranteed claim.
The simpler initial design should avoid that transfer. An alternative is to
reserve all member maximum-window table spans, including lifetime/retry bounds;
that is substantially more expensive and is not selected without a demonstrated
need.

Until the mapping cleanup and ledger conservation tests establish this invariant,
`g` table pages must not be presented as a proven lifetime guarantee.

### Authorization, transactions, and return

A prepared ledger permit binds the incarnation, entitlement, page claim, and
resource authorization before reserved backing is borrowed. It is not an
allocator-supplied claim that an arbitrary range is free.

Placement certification must distinguish ordinary elastic sources from reserved
sources authorized by that permit. Extend the validation boundary with a
reservation witness or equivalent checked ownership, rather than globally making
`Guaranteed` ranges ordinary free memory. Validate alignment, containment,
non-overlap, origin, resource totals, and the permit's entitlement.

A growth crossing the guarantee/elastic boundary is one externally atomic
transaction. It must not commit a guaranteed prefix and then fail its elastic
suffix. The transaction receipt tracks both sources and all new mappings.
Commit follows successful acquisition, placement validation, and mapping.

On rollback or task reclamation:

- Successfully revoked guaranteed children return to their origin reservation.
- Ordinary elastic backing returns to its ordinary owner.
- Failed cleanup remains quarantined and unavailable; it does not produce a
  ledger refund or a reusable source.
- A table still installed in a live address space remains owned and charged.
- Resource exhaustion produces an explicit refusal, never borrowing from an
  unrelated entitlement or operational reservation.

Failure can reduce usable capacity through quarantine. The guarantee does not
claim that resources whose safe reclamation failed are free; recovery must
retain their identity and retry cleanup before refund.

### Boot staging and other commitments

The boot sequence is:

1. Materialize declared operational reservations and every entitlement pool
   before any task is constructed, and admit the resulting ownership partition.
2. Construct autostart tasks with their admitted adaptive address windows,
   binding each incarnation before that task is published.
3. Publish/activate only after every required binding succeeds.

The ownership partition subtracts each guarantee once. The ledger is admitted
against the inventory left after reservation, with each reserved envelope
restored before the ledger's own guarantee subtraction; subtracting an
envelope from a residual that no longer contains it would strand that memory
in neither the elastic pool nor any guarantee.

Reservation precedes construction deliberately: a guarantee that static task
construction can defeat is not a guarantee. The cost is that construction may
now fail for want of residual memory, which fails the boot closed rather than
admitting an unfundable promise. Reported residual capacity is a report, never
a promise.

Internal `TaskTable` construction records are staging state, not publication:
no dispatcher, console thread, or ancillary registry may expose them before
binding. No component may observe a partially admitted adaptive graph or perform
growth between staging and binding. Failure before activation fails the boot
closed; failed cleanup retains ownership in a fail-stop boot, not a claimed
runtime retry loop.

Staged VSpace upper tables and policy-sized leaf-span ownership records are
static commitments consumed before the residual inventory snapshot. Count their
metadata once, separately from the guaranteed payload and lazy leaf-table pool.

Future dynamic task and shared-buffer allocations use ordinary residual memory
or their own explicitly declared operational reservations. Private guarantees
are not an ambient fallback pool. If a future task or shared capacity is itself
promised, reserve its complete backing separately and exclude it from elastic
inventory. If it is best-effort, report allocation refusal instead of describing
it as guaranteed.

Task reservations must use each actual image's VSpace plan, declared CNode and
thread count, rounded arena size, copied capability slots, and metadata. One
materialized exemplar is not a bound for heterogeneous tasks. Shared reservations
must include retained buddy roots, anchors, aliases, and mapping tables, not only
payload pages. Ownership transfer between these pools requires explicit policy
and accounting; this proposal grants no implicit swap authority.

## Alternatives and trade-offs

- **Scalar resource floors:** small implementation surface, but insufficient
  physical-placement isolation and easy to bypass through metadata or ordinary
  allocation. Rejected as proof of a guarantee.
- **Placement-aware floors:** preserve a fit witness after every allocation,
  split, merge, metadata expansion, and rollback. Potentially less idle physical
  ownership, but a much broader mutation boundary than physical reservations.
- **Protected large-frame layouts:** retain large-frame efficiency for
  guarantees, but must accommodate arbitrary incremental requests and cohort
  distribution without stranding backing. More complex than the selected
  base-page lane.
- **Per-entitlement physical pools:** explicit isolation and reusable ownership;
  costs idle backing, reserved capabilities, metadata, and more transaction
  bookkeeping. Guarantees intentionally pay this cost; elastic maxima do not.

## Consequences and required evidence

The reservation mechanism, the base-page guaranteed lane, source-aware
placement certification, reserve-first admission, binding before publication and
source-aware retirement are implemented in `slime-root` and
`boot-contracts/src/private_memory_policy/`, and are exercised on both QEMU
reference architectures by `just private_memory_adaptive_check`. This remains a
QEMU envelope and qualifies no physical machine. The MMIO and device-queue
destination refusals inside a private window are exercised there by a holder
with real device authority. Injected failures prove that a construction
failing after binding releases its incarnation only after the unwind's revoke,
and that a live incarnation whose revoke fails is quarantined rather than
refunded until one retry returns it. The multi-inventory qualification matrix
is out of scope here.

This decision records a cross-module change, not a claim of device
qualification. It requires allocator reservation ownership, ledger permits,
source-aware placement validation, explicit mapping plans, staged binding, and
source-aware reclamation. In-memory bookkeeping does not create a new serialized
format; any new persisted or cross-process representation still requires a
versioned Zutai contract.

Verification must establish:

- ordinary fragmentation after admission cannot defeat a remaining guarantee;
- cohort members can alternate small requests without stranding one another;
- exhausting elastic, shared, or dynamic task allocations does not consume a
  private reservation;
- every admission cost includes funded metadata and capability overhead;
- mixed-source growth fails atomically;
- repeated failed growth cannot leak refunded guarantee-funded tables;
- failed revoke preserves quarantine and prevents premature refund/reuse;
- teardown and incarnation replacement return backing to the correct entitlement;
- existing fixed-policy and elastic large-frame qualification remains valid;
- no adaptive task activates with an absent binding or partial admission.

These are required observations, not results recorded by this document.

## Revisit conditions

Revisit the base-page lane if measured capability/metadata cost makes useful
guarantees inadmissible, or if a protected large-frame strategy proves both
arbitrary incremental growth and cohort fairness. Revisit subdivision only after
its resource bound and failure conservation are independently tested. Revisit
operational partitioning when new task, shared-memory, DMA, or kernel commitments
are introduced. None of these changes may weaken physical isolation or substitute
aggregate byte counts for a placement proof.
