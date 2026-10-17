# Console record qualification

This is the exam design for backlog
[`01a11bda-7038-7b80-8963-ae49bf9ce922`](../../.tasks/items/01a11bda-7038-7b80-8963-ae49bf9ce922.md),
not a claim that the product already serializes records.

## Landed-exam boundary

`just console_record_check` first runs deterministic actual-source contention,
then twenty consecutive executions of the existing local-network QEMU path.
The checker, its embedded Rust adapter, controls, recipe, and execution inputs
must land before the repair is graded. The Rust subject comes from the working
tree; the adapter lives inside the immutable Python grader closure. No product
implementation is supplied by this planning change.

The host adapter compiles the exact `console::write_payload` function and the
actual backing snapshot-begin formatting invocation, together with upstream
seL4's byte-wise formatter. The repaired shared `slime-root/src/diagnostic.rs`
is compiled unchanged when present. Kernel byte emission and yield operations
are replaced by host observation hooks; staging is supplied by bounded fixture
inputs rather than performing kernel mappings. No host mutex supplies product
record exclusion: its mutex protects only the captured byte stream and fixture
coordination. Product ownership decisions must execute in the included module.

After producer A emits its first byte, it is parked. Producer B must either
emit a byte (the unprotected baseline) or reach the actual product algorithm's
contended kernel-yield operation before the controller releases A. Merely
starting B or sleeping is insufficient. Eight rounds of both owner orders and
normal, non-UTF-8 and staging-refusal console output require byte-exact contiguous
records and bounded completion. Missing-window, unavailable-frame and oversized
staging diagnostics compete in both ownership orders, not just isolated checks. Missing synchronization and a non-yielding busy wait are independently
refused by synthetic sensitivity controls. Their positive subject is an exam
control, never evidence about repaired product code.

Repeated acquisition is also forced deterministically: a queued contender remains
parked while the current owner completes and immediately submits another record.
The second submission must yield behind that earlier contender; byte-exact order
is owner-first, contender, owner-second. Both ownership directions are exercised.
A yielding but unfair test-and-set control must fail this handoff even if it passes
all single-record exclusion cases. Fixture observation hooks control scheduling,
not product lock ownership.

Whole invalid UTF-8, oversized, unavailable staging, missing-window and short
message refusals must emit only their independently expected refusal diagnostic
or no output, never an accepted payload prefix. Formatting must complete
within the bounded buffer before any bytes are emitted. Console refusal diagnostics
also compete through the same output boundary. The authenticated console dispatch fixture also compiles the actual badge decoder,
per-holder/thread window lookup, and Write dispatch route. Invalid badges and staged
writes without their own bound window are refused; inline writes retain the existing
kernel-minted endpoint authority rather than inventing a new window requirement.
Kernel mapping/authority mechanics are not replaced by a new host claim: their
existing root and plane gates remain.

## Local producer operations

A separate actual-source fixture records individual `debug_write` submissions,
not just their concatenation. The affected probe's role/traffic/teardown, numeric
and failure lines, and the network service's local summary lines must each match
an independently expected complete line in one bounded operation. A correct
concatenation split into multiple messages still fails. Failure paths are exercised
explicitly rather than inferred from twenty successful boots. A shared bounded
producer line helper, when introduced, is compiled from its working-tree source;
the grader does not supply a replacement implementation. A narrow lexical caller
and prefix audit connects the tested helpers/blocks to the actual local component
call sites and rejects parallel qualified-prefix emitters; it is not a formal
reachability proof. Inline replacement blocks retain their marker literal and local
inputs in one `Line::<N>::new().bytes(...).decimal(...).emit()` expression. This
fixed seam is part of the exam, not permission to add unused host-only helpers.
Conditional target/test implementations are refused in the qualified producer
sources and the shared line helper: the host compiler must not select a different
record implementation from the AArch64 seL4 image. The existing external-only
fault injector's architecture-specific assembly is the sole exception: its exact
body is frozen, and conditional attributes enclosing it are still refused.

## Frozen repair seam and scheduling scope

The exam expects one library-owned diagnostic module, with `write(&[u8])` returning
a refusal on invalid/overbound bytes and `print(core::fmt::Arguments)` formatting
before output. Valid 1,024-byte direct, staged and formatted records must be
accepted byte-exact; rejecting 1,025 bytes is not permission to narrow the envelope.
The existing console writer calls that same `write` route; root formatting uses
the module's exported diagnostic macros. The binary must not
instantiate a second module with separate ownership state. The 1,024-byte envelope
is the existing staged-array bound, not a new serialized record format.

A lexical source audit refuses raw root debug macros, byte output and writer
aliases outside the shared module. Inside it, `write` and `print` are the only
published functions, the kernel byte sink appears exactly once inside `write`, and
root callers may name only those two entry points (or the record macros), so an
unlocked alternate sink cannot carry a different producer. The whole implementation
stays in that one audited file: file-backed submodules and crate-local helpers are
refused, so no unaudited child can select a different lock per target. It also requires actual native yield and byte
operations, with no host-only or test-only implementation branch. This audit is
not arbitrary Rust data-flow analysis or a formal proof of all macro expansion.

The progress claim is limited to the existing two root producers on the pinned
single-core, non-MCS AArch64 QEMU target: root and console run at maximum priority
255. Non-MCS `handleYield` appends the current runnable thread to the ready queue,
allowing an equally prioritized runnable owner to continue. FIFO ownership can
bound waiting by the preceding bounded record. No lower-priority owner, additional
root producer, SMP/MCS target, recursive formatting/output, root-fault recovery or
power-loss durability is qualified by this exam. Kernel-generated diagnostics
are outside userspace output ownership.

The owning runtime `debug_write` documentation is checked as part of the frozen
contract audit. It must state that each call submits one bounded payload to the
console dispatcher, callers assemble complete records without fragment joining,
root and console share serialization on the qualified single-core non-MCS path,
and send completion does not acknowledge emission. The obsolete single-threaded
graph-loop atomicity explanation cannot remain a qualifying public contract.
The transport implementation is not a repair surface: fixed source digests freeze
the native sender outside that editable doc block, its public syscall dispatch,
wire helpers, and the generated runtime ABI constant provider/wrapper. This is a
source-preservation boundary, not a claim that the host fixture executes seL4 IPC.
A producer capture stub cannot qualify a sender that splits calls into multiple
IPC messages or narrows the staging capacity.

## QEMU regression and raw evidence

The local checker retains every original `LOCAL_CHAINS` and `FAILURE_MARKERS`
assertion. The repeated qualification additionally requires complete local probe/service
records with their declared multiplicity and exactly one final HEALTHY census;
an intact marker cannot hide an extra broken or prohibited duplicated record.
Service coverage includes all sixteen normal records on the local plane, not
only its three original summary markers: authority, ceilings, incarnation,
per-application options, congestion control, interface, accepted/listener close,
per-application traffic and buffer release, wait/frames and observed totals.
Identical per-holder records are counted rather than incorrectly required to be
unique. Only service-local causal order is added; independent peer arrivals stay
unconstrained. Exceptional invalidation, abort and failure output is tested at the
producer-operation boundary and cannot masquerade as normal local qualification. Controls reject missing, split, reversed,
changed, duplicated and explicit-failure evidence. No marker reconstruction or
retry-to-green is permitted.

The repeated arm builds once, checks the same image digest before each boot, and
writes exclusive ordinal `.serial` files under the printed unique
`build/console-record-*` directory. These contain original collected QEMU bytes,
including CRLF and partial failed-run tails. Collection ends at the terminal line;
post-terminal output is not drained. Failure aborts the sequence and leaves prior
and failed captures in place. Without opt-in raw capture, existing `run_plane`
text-mode behavior is unchanged.

Twenty passing boots are a regression envelope, not the proof of atomicity.
The deterministic actual-source case is mandatory and the baseline must fail.
`just console_record_controls` only judges exam sensitivity and cannot supply
real-item qualification evidence. Raw captures are local diagnostics, not a new
portable evidence format or an upload instruction.
