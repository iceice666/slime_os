# `sel4-stream.zti` — the P5.5.2 stream-plane generation

A seventh seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md), and the
frozen x86 [`valid.zti`](valid.zti). It declares the graph that carries P5.5's
exit condition in full: the C8.4 stream plane as the x86 oracle builds it, with
every participant unmodified.

It **replaces** P5.5.1's `sel4-fabric.zti`, which is deleted rather than kept.
That manifest declared one route, one publisher, and one subscriber — the
smallest graph that could carry a C8.3 authority claim. Every property it
observed is a property this graph also observes, over a larger composition, so
keeping both would have meant maintaining two images and two gates to see the
same thing twice. The retirement is recorded in the roadmap under P5.5.1 rather
than left implicit.

## Why a seventh generation

The same mechanical reason as the five before it: `init.rs` selects its scenario
with `option_env!`, resolved at compile time, so one component build cannot
serve two gates.

## The six participants, all unmodified

`fabric-service`, `fabric-publisher`, `fabric-publisher-b`,
`fabric-subscriber`, `fabric-subscriber-b`, and `fabric-intruder` are the same
binaries the x86 oracle builds, with **no seL4 branch in any of them**. The gate
asserts that at the source rather than inferring it from the transcript.

That is the difference from P5.5.1, and it is the milestone. That slice's
`fabric-subscriber` carried exactly one branch: it refuses to finish until both
sample forms arrive, and the `>MAX_MSG` one comes from `fabric-publisher-b`,
which a one-publisher graph does not declare. The branch was removed here by
**declaring that publisher**, which is what P5.5.1's own comment said the way
back would be — the component was not edited to suit the new graph.

## What the second publisher and second subscriber are for

Neither is scaffolding. Each carries a clause of the C8.4 exit condition that a
one-publisher, one-subscriber graph cannot reach:

- **`fabric-publisher-b`** originates the `>MAX_INLINE_BYTES` sample. That is
  the C7.6 path — a quota-charged shared buffer, sealed irreversibly, loaned to
  the fabric by capability, and re-loaned once per matched subscriber — and it
  is what makes "one copy per large sample, one loan per subscriber" an
  observable count rather than an argument. It also spans **both** routes, which
  is what makes the fan-in many-to-many rather than two independent lines.
- **`fabric-subscriber-b`** stalls: it consumes, then deliberately stops
  acking. Its declared `historyDepth = 4` against the publishers' seven samples
  is what gives KEEP_LAST eviction an observable cost, and its resumption
  produces the bounded `SAMPLE_LOST` report. It also subscribes to
  `diagnostics`, which is how "one participant's stall does not disturb an
  unrelated stream" is checked rather than assumed.

`fabric-intruder` is spawned holding a real control endpoint on purpose, as it
was in P5.5.1: the denial under test is not "no channel" but "no declared edge".

## B17's subject, and why it is a spawn grant

`fabric-publisher` additionally receives a **second** endpoint end, granted at
`send`+`transfer`. It is not part of any route and carries no traffic; it exists
so the transfer contract's **subset test** has a subject.

It goes to the publisher because that component already carries this graph's
other two transfer-rule denials — the re-delegation and the per-kind widening —
so all three sit together and each states which rule it proves.

That test — `rights & !source.rights` — was recorded as uncovered by P5.5.1,
and the backlog's stated reason was that only a `cap_transfer` retaining its
transfer bit could produce a capability holding transfer authority while being
narrower than its kind admits. **That reason was wrong.** A plain spawn grant
produces one: `preflight_spawn_grants` installs the requested mask verbatim, so
`grant(endpoint, RIGHT_SEND | RIGHT_TRANSFER)` yields exactly send+transfer
where `Endpoint` admits send+recv+transfer. Init already does this on x86 for
`DANGO_OUTPUT_SLOT`; nobody had asked to widen one.

Asking to move that end with `recv` restored passes the transfer-authority rule
(the bit is there), passes the descriptor/kind rule, and computes zero against
the per-kind mask. Only the subset test refuses it — verified by deleting the
test and watching this gate fail, which is what P5.5.1's graph could not do.

The arm is guarded on **holding** the subject rather than on a check flag,
because an empty slot answers the same `ERR_BAD_CAP` the subset test does: a
bare widening arm would pass identically in a graph that never granted the
endpoint. `valid.zti` grants no probe, so the arm skips silently there, and the
gate records that as its one declared seL4-only marker.

## The numbers, and why each

`init` declares `spawnBudget = 6` — exactly the six children this composition
needs, and not also a denial arm; B14's refusal is already observed in
[`sel4-sample.zti`](sel4-sample.md).

The QoS rows, history depths, and shared-buffer budgets are **copied from
`valid.zti`** rather than chosen, for the reason P5.5.1 recorded after picking
its own: the participants' behaviour is tuned to the oracle's numbers, and a
depth chosen here to look reasonable made a keeping-up subscriber lose samples
it had already acked. The oracle's numbers are the right ones for the oracle's
publishers.

`capabilitySlots = 32` against the x86 profile's 48, because this graph declares
no call or operation plane and `requiredCapabilitySlots` is summed per plane.

## What this graph made the root grow

Two changes in `slime-root`, both of which this composition was the first to
demand:

- **`MAX_CHANNELS` 16 → 32.** The old bound's stated reasoning was "one channel
  per task pair", which this graph disproves: channels are created per *edge*,
  and a userspace broker mints edges the generation never declared. Thirteen
  declared grants become six control channels, and the fabric then mints two per
  publisher and two per subscriber. At sixteen the fabric failed its eleventh
  `endpoint_create`, and every participant failed downstream of that — which
  reads as four broken components rather than one exhausted table.
- **`shared_buffer_unmap` accepts a loan slot.** The retired kernel's
  `sys_shared_buffer_unmap` resolves `SharedBufferLoan(loan) => loan.region()`;
  the root's resolved only a `SharedBuffer`, so a receiver that mapped through
  `loan_map` was answered `ERR_BAD_CAP` on the only slot it holds — the region
  belongs to the lender. Latent since P5.3.2 and unreachable until a component
  actually mapped a downstream loan, which is exactly what the shared-sample
  path does. The third such ABI divergence this cutover has found, after the two
  P5.5.1 fixed.

## What the root still launches

The root launches every component the generation declares (P5.2), so this boot
also starts one unconfigured instance of each of the six, holding no control
endpoint. Each fails its own first operation and exits non-zero.

That is expected, and the gate handles it by **identity rather than by time**,
for the reason P5.5.1's gate recorded: the unconfigured instances are activated
alongside init's six children and interleave freely with them, so a transcript
window would admit a real failure or exclude one depending on scheduling. The
gate counts instead — each component name must appear with exactly one failure,
which is the unconfigured instance's, so a second from the same name is
necessarily a participant's.

## Relationship to the backlog

- **B17 is closed** by this slice, and its entry's reasoning is corrected rather
  than merely marked resolved: the premise that no declared graph could produce
  the subject was false.
- **B16** (termination records are never reclaimed) does not bite: this graph
  creates thirteen tasks against `MAX_RECORDS = 32`.
