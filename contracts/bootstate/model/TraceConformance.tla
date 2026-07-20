-------------------------- MODULE TraceConformance --------------------------
\* M5.6c BootState model-implementation conformance oracle.
\*
\* This module EXTENDS the checked M5.6a/M5.6b `BootState` model, so `Init`,
\* `Next`, and every transition rule are the SAME single source of truth the
\* main model check validates. It adds no transition of its own: it only
\* describes the durable post-state a single observed trace record claims and
\* lets TLC decide whether that post-state is reachable in the real model.
\*
\* `scripts/check-bootstate-trace.py` drives it with the `Obs*` constants of one
\* trace record and the witness invariant `NoObservedReach`:
\*   * a conformant record's post-state IS reachable, so TLC reports the
\*     invariant violated (accepted);
\*   * a non-conformant record (e.g. an attempt that was not durably
\*     decremented before transfer) has NO reachable matching state, so TLC
\*     finds no violation (rejected).
\*
\* Concrete generation/root identities and absolute sequence numbers are
\* values the abstract model does not carry; the checker binds those to the
\* on-disk BootState and validates their structural deltas. This module owns
\* the abstract transition legality: action/commit correspondence, attempt
\* arithmetic, and the pending/known-good presence transition.

EXTENDS BootState

CONSTANTS ObsAction, ObsCommit, ObsAttemptsBefore, ObsAttemptsAfter

\* True when a reachable model state exhibits the durable post-state the
\* observed record claims for its action.
ObservedReached ==
    CASE ObsAction = "consume-attempt" ->
            \* Stage-0 durably decremented the pending attempt into the
            \* committed slot before transferring control.
            /\ ObsCommit = "after-attempt-commit"
            /\ phase = "attempt-committed"
            /\ writeStage = "idle"
            /\ attemptsBeforeTransfer = ObsAttemptsBefore
            /\ SelectedSlot \in SlotNames
            /\ SelectedState.pending # NoGeneration
            /\ SelectedState.remainingAttempts = ObsAttemptsAfter
      [] ObsAction = "promotion" ->
            \* Health confirmation promoted the running pending generation to
            \* known-good, cleared pending, and retained a rollback root.
            /\ ObsCommit = "health-promotion"
            /\ phase = "running"
            /\ runningPending = FALSE
            /\ SelectedSlot \in SlotNames
            /\ SelectedState.pending = NoGeneration
            /\ SelectedState.remainingAttempts = ObsAttemptsAfter
            /\ rollbackRoots # {}
      [] ObsAction = "boot-known-good" ->
            \* Booted the known-good generation with no pending present.
            /\ ObsCommit = "none"
            /\ phase = "running"
            /\ runningPending = FALSE
            /\ SelectedSlot \in SlotNames
            /\ SelectedState.pending = NoGeneration
      [] ObsAction = "boot-exhausted-known-good" ->
            \* Booted the known-good generation after pending attempts were
            \* durably exhausted; the pending record survives with zero
            \* attempts until an explicit rollback clears it.
            /\ ObsCommit = "none"
            /\ phase = "running"
            /\ runningPending = FALSE
            /\ SelectedSlot \in SlotNames
            /\ SelectedState.pending # NoGeneration
            /\ SelectedState.remainingAttempts = ObsAttemptsAfter
      [] OTHER -> FALSE

\* Witness invariant: TLC violates it exactly when the observed post-state is
\* reachable, which the checker reads as "accepted by the model".
NoObservedReach == ~ObservedReached

=============================================================================
