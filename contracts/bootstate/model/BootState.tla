------------------------------- MODULE BootState -------------------------------
EXTENDS Integers, FiniteSets, TLC

CONSTANTS Generations, NoGeneration, StateRoots, NoStateRoot,
          MaxAttempts, MaxSequence, RequiredCut

ASSUME /\ NoGeneration \notin Generations
       /\ NoStateRoot \notin StateRoots
       /\ Generations \cap StateRoots = {}

SlotNames == {"A", "B"}

EmptyRecord == [
    valid |-> FALSE,
    sequence |-> 0,
    knownGood |-> NoGeneration,
    pending |-> NoGeneration,
    remainingAttempts |-> 0,
    generationSet |-> {},
    stateRoot |-> NoStateRoot
]

RecordType == [
    valid : BOOLEAN,
    sequence : 0..MaxSequence,
    knownGood : Generations \cup {NoGeneration},
    pending : Generations \cup {NoGeneration},
    remainingAttempts : 0..MaxAttempts,
    generationSet : SUBSET Generations,
    stateRoot : StateRoots \cup {NoStateRoot}
]

RecordConsistent(record) ==
    /\ record \in RecordType
    /\ record.knownGood \in Generations
    /\ record.knownGood \in record.generationSet
    /\ record.pending \in Generations \cup {NoGeneration}
    /\ record.pending # NoGeneration => record.pending \in record.generationSet
    /\ record.pending = NoGeneration => record.remainingAttempts = 0
    /\ record.remainingAttempts \in 0..MaxAttempts
    /\ record.stateRoot \in StateRoots

VARIABLES slots,
          durableGenerations, durableStateRoots, rollbackRoots,
          phase, running, runningPending,
          writeTarget, writeValue, writeKind, writeStage,
          attemptsBeforeTransfer, lastCut,
          stagedCandidate, snapshotCandidate, gcCandidate

vars == <<slots,
          durableGenerations, durableStateRoots, rollbackRoots,
          phase, running, runningPending,
          writeTarget, writeValue, writeKind, writeStage,
          attemptsBeforeTransfer, lastCut,
          stagedCandidate, snapshotCandidate, gcCandidate>>

OtherSlot(slot) == IF slot = "A" THEN "B" ELSE "A"

SelectedSlot ==
    IF slots["A"].valid /\ slots["B"].valid THEN
        IF slots["A"].sequence > slots["B"].sequence THEN "A"
        ELSE IF slots["B"].sequence > slots["A"].sequence THEN "B"
        ELSE IF slots["A"] = slots["B"] THEN "A"
        ELSE "none"
    ELSE IF slots["A"].valid THEN "A"
    ELSE IF slots["B"].valid THEN "B"
    ELSE "none"

SelectedState == IF SelectedSlot \in SlotNames THEN slots[SelectedSlot] ELSE EmptyRecord

OlderSlot ==
    IF slots["A"].valid /\ ~slots["B"].valid THEN "B"
    ELSE IF slots["B"].valid /\ ~slots["A"].valid THEN "A"
    ELSE IF slots["A"].valid /\ slots["B"].valid THEN
        IF slots["A"].sequence < slots["B"].sequence THEN "A" ELSE "B"
    ELSE "none"

BootableRecord(record) ==
    /\ record.valid
    /\ RecordConsistent(record)
    /\ record.generationSet \subseteq durableGenerations
    /\ record.knownGood \in durableGenerations
    /\ record.pending # NoGeneration => record.pending \in durableGenerations
    /\ record.stateRoot \in durableStateRoots

ValidSlots == {slot \in SlotNames : slots[slot].valid}
ValidGenerationRefs == UNION {slots[slot].generationSet : slot \in ValidSlots}
ValidStateRefs == {slots[slot].stateRoot : slot \in ValidSlots}
ProtectedIdentities ==
    ValidGenerationRefs \cup ValidStateRefs \cup rollbackRoots
    \cup (IF running = NoGeneration THEN {} ELSE {running})
    \cup (IF stagedCandidate = NoGeneration THEN {} ELSE {stagedCandidate})
    \cup (IF snapshotCandidate = NoStateRoot THEN {} ELSE {snapshotCandidate})

TypeOK ==
    /\ slots \in [SlotNames -> RecordType]
    /\ durableGenerations \subseteq Generations
    /\ durableStateRoots \subseteq StateRoots
    /\ rollbackRoots \subseteq Generations
    /\ phase \in {"boot", "running", "attempt-committed", "pending-committed", "snapshot", "gc"}
    /\ running \in Generations \cup {NoGeneration}
    /\ runningPending \in BOOLEAN
    /\ writeTarget \in SlotNames \cup {"none"}
    /\ writeValue \in RecordType
    /\ writeKind \in {"none", "stage-pending", "consume-attempt", "promotion", "rollback"}
    /\ writeStage \in {"idle", "prepared", "invalidated", "payload-written", "flushed"}
    /\ attemptsBeforeTransfer \in 0..MaxAttempts
    /\ lastCut \in {"none", "before-pending-metadata", "slot-write-A", "slot-write-B",
                     "after-pending-commit", "after-attempt-commit", "health-promotion",
                     "rollback-update", "state-snapshot", "garbage-collection"}
    /\ stagedCandidate \in Generations \cup {NoGeneration}
    /\ snapshotCandidate \in StateRoots \cup {NoStateRoot}
    /\ gcCandidate \in Generations \cup StateRoots \cup {NoGeneration, NoStateRoot}

Init ==
    LET initial == [
        valid |-> TRUE,
        sequence |-> 1,
        knownGood |-> CHOOSE generation \in Generations : TRUE,
        pending |-> NoGeneration,
        remainingAttempts |-> 0,
        generationSet |-> {CHOOSE generation \in Generations : TRUE},
        stateRoot |-> CHOOSE root \in StateRoots : TRUE
    ]
    IN /\ Cardinality(Generations) = 3
       /\ Cardinality(StateRoots) = 2
       /\ MaxAttempts = 2
       /\ MaxSequence = 8
       /\ slots = [slot \in SlotNames |-> IF slot = "A" THEN [initial EXCEPT !.sequence = 2] ELSE initial]
       /\ durableGenerations = Generations
       /\ durableStateRoots = {initial.stateRoot}
       /\ rollbackRoots = {}
       /\ phase = "boot"
       /\ running = NoGeneration
       /\ runningPending = FALSE
       /\ writeTarget = "none"
       /\ writeValue = EmptyRecord
       /\ writeKind = "none"
       /\ writeStage = "idle"
       /\ attemptsBeforeTransfer = 0
       /\ lastCut = "none"
       /\ stagedCandidate = NoGeneration
       /\ snapshotCandidate = NoStateRoot
       /\ gcCandidate = NoGeneration

BootKnownGood ==
    /\ phase = "boot"
    /\ writeStage = "idle"
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)
    /\ SelectedState.pending = NoGeneration
    /\ phase' = "running"
    /\ running' = SelectedState.knownGood
    /\ runningPending' = FALSE
    /\ attemptsBeforeTransfer' = 0
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    writeTarget, writeValue, writeKind, writeStage, lastCut,
                    stagedCandidate, snapshotCandidate, gcCandidate>>

BootExhaustedKnownGood ==
    /\ phase = "boot"
    /\ writeStage = "idle"
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)
    /\ SelectedState.pending # NoGeneration
    /\ SelectedState.remainingAttempts = 0
    /\ phase' = "running"
    /\ running' = SelectedState.knownGood
    /\ runningPending' = FALSE
    /\ attemptsBeforeTransfer' = 0
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    writeTarget, writeValue, writeKind, writeStage, lastCut,
                    stagedCandidate, snapshotCandidate, gcCandidate>>

BeginStagePending ==
    /\ phase = "running"
    /\ runningPending = FALSE
    /\ SelectedSlot \in SlotNames
    /\ running = SelectedState.knownGood
    /\ SelectedState.pending = NoGeneration
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ stagedCandidate = NoGeneration
    /\ \E candidate \in durableGenerations \ SelectedState.generationSet:
          stagedCandidate' = candidate
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeValue,
                    writeKind, writeStage, attemptsBeforeTransfer, lastCut,
                    snapshotCandidate, gcCandidate>>

StartPendingMetadata ==
    /\ phase = "running"
    /\ runningPending = FALSE
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.pending = NoGeneration
    /\ stagedCandidate \in durableGenerations
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ writeValue' = [SelectedState EXCEPT
                        !.pending = stagedCandidate,
                        !.remainingAttempts = MaxAttempts,
                        !.generationSet = @ \cup {stagedCandidate}]
    /\ writeKind' = "stage-pending"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

AttemptAfterConsume(record) == [record EXCEPT !.remainingAttempts = @ - 1]
SkipAttempt(record) == record

PreparePendingAttempt ==
    /\ phase = "boot"
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)
    /\ SelectedState.pending # NoGeneration
    /\ SelectedState.remainingAttempts > 0
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ attemptsBeforeTransfer' = SelectedState.remainingAttempts
    /\ writeValue' = AttemptAfterConsume(SelectedState)
    /\ writeKind' = "consume-attempt"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeStage,
                    lastCut, stagedCandidate, snapshotCandidate, gcCandidate>>

BeginHealthPromotion ==
    /\ phase = "running"
    /\ runningPending = TRUE
    /\ SelectedSlot \in SlotNames
    /\ running = SelectedState.pending
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ writeValue' = [SelectedState EXCEPT
                        !.knownGood = running,
                        !.pending = NoGeneration,
                        !.remainingAttempts = 0]
    /\ writeKind' = "promotion"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

BeginRollback ==
    /\ phase = "running"
    /\ SelectedSlot \in SlotNames
    /\ \/ runningPending
       \/ /\ ~runningPending
          /\ running = SelectedState.knownGood
          /\ SelectedState.pending # NoGeneration
          /\ SelectedState.remainingAttempts = 0
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ writeValue' = [SelectedState EXCEPT
                        !.pending = NoGeneration,
                        !.remainingAttempts = 0]
    /\ writeKind' = "rollback"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

PrepareWrite(kind, value) ==
    /\ writeStage = "idle"
    /\ kind \in {"stage-pending", "consume-attempt", "promotion", "rollback"}
    /\ value = writeValue
    /\ kind = writeKind
    /\ SelectedSlot \in SlotNames
    /\ OlderSlot \in SlotNames
    /\ OlderSlot # SelectedSlot
    /\ OtherSlot(OlderSlot) = SelectedSlot
    /\ SelectedState.sequence < MaxSequence
    /\ RecordConsistent(value)
    /\ BootableRecord(slots[OtherSlot(OlderSlot)])
    /\ writeTarget' = OlderSlot
    /\ writeValue' = value
    /\ writeKind' = kind
    /\ writeStage' = "prepared"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, attemptsBeforeTransfer,
                    lastCut, stagedCandidate, snapshotCandidate, gcCandidate>>

PrepareWriteStep == PrepareWrite(writeKind, writeValue)

InvalidateWriteTarget ==
    /\ writeStage = "prepared"
    /\ writeTarget \in SlotNames
    /\ slots' = [slots EXCEPT ![writeTarget].valid = FALSE]
    /\ writeStage' = "invalidated"
    /\ UNCHANGED <<durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeValue,
                    writeKind, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

WritePayload ==
    /\ writeStage = "invalidated"
    /\ writeTarget \in SlotNames
    /\ slots' = [slots EXCEPT ![writeTarget] = [writeValue EXCEPT !.valid = FALSE]]
    /\ writeStage' = "payload-written"
    /\ UNCHANGED <<durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeValue,
                    writeKind, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

FlushPayload ==
    /\ writeStage = "payload-written"
    /\ writeStage' = "flushed"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    phase, running, runningPending, writeTarget, writeValue,
                    writeKind, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

CommitWrite ==
    /\ writeStage = "flushed"
    /\ writeTarget \in SlotNames
    /\ SelectedSlot = OtherSlot(writeTarget)
    /\ slots' = [slots EXCEPT
                   ![writeTarget] = [writeValue EXCEPT
                                      !.sequence = SelectedState.sequence + 1,
                                      !.valid = TRUE]]
    /\ rollbackRoots' = IF writeKind = "promotion"
                         THEN rollbackRoots \cup {SelectedState.knownGood}
                         ELSE rollbackRoots
    /\ phase' = CASE writeKind = "stage-pending" -> "pending-committed"
                  [] writeKind = "consume-attempt" -> "attempt-committed"
                  [] writeKind = "promotion" -> "running"
                  [] writeKind = "rollback" -> "boot"
    /\ running' = IF writeKind = "rollback" THEN NoGeneration ELSE running
    /\ runningPending' = IF writeKind \in {"promotion", "rollback"}
                          THEN FALSE ELSE runningPending
    /\ writeTarget' = "none"
    /\ writeValue' = EmptyRecord
    /\ writeKind' = "none"
    /\ writeStage' = "idle"
    /\ UNCHANGED <<durableGenerations, durableStateRoots,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

RebootAfterPendingCommit ==
    /\ phase = "pending-committed"
    /\ writeStage = "idle"
    /\ phase' = "boot"
    /\ running' = NoGeneration
    /\ runningPending' = FALSE
    /\ stagedCandidate' = NoGeneration
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    writeTarget, writeValue, writeKind, writeStage,
                    attemptsBeforeTransfer, lastCut, snapshotCandidate, gcCandidate>>

TransferPending ==
    /\ phase = "attempt-committed"
    /\ writeStage = "idle"
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.pending # NoGeneration
    /\ phase' = "running"
    /\ running' = SelectedState.pending
    /\ runningPending' = TRUE
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    writeTarget, writeValue, writeKind, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

BeginSnapshot ==
    /\ phase = "running"
    /\ snapshotCandidate = NoStateRoot
    /\ \E candidate \in StateRoots \ durableStateRoots:
          snapshotCandidate' = candidate
    /\ phase' = "snapshot"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    running, runningPending, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    gcCandidate>>

WriteSnapshot ==
    /\ phase = "snapshot"
    /\ snapshotCandidate \in StateRoots
    /\ UNCHANGED vars

SealSnapshot ==
    /\ phase = "snapshot"
    /\ snapshotCandidate \in StateRoots
    /\ durableStateRoots' = durableStateRoots \cup {snapshotCandidate}
    /\ snapshotCandidate' = NoStateRoot
    /\ phase' = "running"
    /\ UNCHANGED <<slots, durableGenerations, rollbackRoots, running,
                    runningPending, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    gcCandidate>>

BeginGc ==
    /\ phase = "running"
    /\ gcCandidate \in {NoGeneration, NoStateRoot}
    /\ \E candidate \in (durableGenerations \cup durableStateRoots) \ ProtectedIdentities:
          gcCandidate' = candidate
    /\ phase' = "gc"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    running, runningPending, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate>>

DeleteGcCandidate ==
    /\ phase = "gc"
    /\ gcCandidate \in durableGenerations \cup durableStateRoots
    /\ durableGenerations' = durableGenerations \ {gcCandidate}
    /\ durableStateRoots' = durableStateRoots \ {gcCandidate}
    /\ UNCHANGED <<slots, rollbackRoots, phase, running, runningPending,
                    writeTarget, writeValue, writeKind, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate, gcCandidate>>

FinishGc ==
    /\ phase = "gc"
    /\ gcCandidate \notin durableGenerations \cup durableStateRoots
    /\ gcCandidate' = NoGeneration
    /\ phase' = "running"
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots,
                    running, runningPending, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut, stagedCandidate,
                    snapshotCandidate>>

Crash(cut) ==
    /\ phase' = "boot"
    /\ running' = NoGeneration
    /\ runningPending' = FALSE
    /\ writeTarget' = "none"
    /\ writeValue' = EmptyRecord
    /\ writeKind' = "none"
    /\ writeStage' = "idle"
    /\ attemptsBeforeTransfer' = 0
    /\ lastCut' = cut
    /\ stagedCandidate' = NoGeneration
    /\ snapshotCandidate' = NoStateRoot
    /\ gcCandidate' = NoGeneration
    /\ UNCHANGED <<slots, durableGenerations, durableStateRoots, rollbackRoots>>

CutBeforePendingAction ==
    /\ phase = "running"
    /\ stagedCandidate \in Generations
    /\ writeKind = "none"
    /\ Crash("before-pending-metadata")

CutSlotWriteAction ==
    /\ writeTarget \in SlotNames
    /\ writeStage \in {"invalidated", "payload-written", "flushed"}
    /\ Crash(IF writeTarget = "A" THEN "slot-write-A" ELSE "slot-write-B")

CutAfterPendingAction ==
    /\ phase = "pending-committed"
    /\ writeStage = "idle"
    /\ Crash("after-pending-commit")

CutAfterAttemptAction ==
    /\ phase = "attempt-committed"
    /\ writeStage = "idle"
    /\ Crash("after-attempt-commit")

CutPromotionAction ==
    /\ writeKind = "promotion"
    /\ writeStage \in {"invalidated", "payload-written", "flushed"}
    /\ Crash("health-promotion")

CutRollbackAction ==
    /\ writeKind = "rollback"
    /\ writeStage \in {"invalidated", "payload-written", "flushed"}
    /\ Crash("rollback-update")

CutSnapshotAction ==
    /\ phase = "snapshot"
    /\ snapshotCandidate \in StateRoots
    /\ Crash("state-snapshot")

CutGcAction ==
    /\ phase = "gc"
    /\ gcCandidate \in Generations \cup StateRoots
    /\ Crash("garbage-collection")

StutterAtSequenceBound ==
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.sequence = MaxSequence
    /\ UNCHANGED vars

Next ==
    \/ BootKnownGood
    \/ BootExhaustedKnownGood
    \/ BeginStagePending
    \/ StartPendingMetadata
    \/ PreparePendingAttempt
    \/ BeginHealthPromotion
    \/ BeginRollback
    \/ PrepareWriteStep
    \/ InvalidateWriteTarget
    \/ WritePayload
    \/ FlushPayload
    \/ CommitWrite
    \/ RebootAfterPendingCommit
    \/ TransferPending
    \/ BeginSnapshot
    \/ WriteSnapshot
    \/ SealSnapshot
    \/ BeginGc
    \/ DeleteGcCandidate
    \/ FinishGc
    \/ CutBeforePendingAction
    \/ CutSlotWriteAction
    \/ CutAfterPendingAction
    \/ CutAfterAttemptAction
    \/ CutPromotionAction
    \/ CutRollbackAction
    \/ CutSnapshotAction
    \/ CutGcAction
    \/ StutterAtSequenceBound

Spec == Init /\ [][Next]_vars

ValidRecordsConsistent ==
    \A slot \in SlotNames : slots[slot].valid => RecordConsistent(slots[slot])

SelectableBootRootExists ==
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)

ProtectedRootsPresent ==
    /\ ValidGenerationRefs \subseteq durableGenerations
    /\ ValidStateRefs \subseteq durableStateRoots
    /\ rollbackRoots \subseteq durableGenerations
    /\ running # NoGeneration => running \in durableGenerations
    /\ stagedCandidate # NoGeneration => stagedCandidate \in durableGenerations

PendingAttemptConsumedBeforeTransfer ==
    ~runningPending
    \/ /\ SelectedSlot \in SlotNames
       /\ SelectedState.pending = running
       /\ SelectedState.remainingAttempts = attemptsBeforeTransfer - 1

ConfirmationOnlyForRunningPending ==
    writeKind # "promotion"
    \/ /\ runningPending
       /\ running = writeValue.knownGood

WritePreservesOtherSlot ==
    writeStage = "idle"
    \/ /\ writeTarget \in SlotNames
       /\ slots[OtherSlot(writeTarget)].valid
       /\ BootableRecord(slots[OtherSlot(writeTarget)])

CutWitnessMissing == lastCut # RequiredCut

CutBeforePending == "before-pending-metadata"
CutSlotA == "slot-write-A"
CutSlotB == "slot-write-B"
CutAfterPending == "after-pending-commit"
CutAfterAttempt == "after-attempt-commit"
CutPromotion == "health-promotion"
CutRollback == "rollback-update"
CutSnapshot == "state-snapshot"
CutGc == "garbage-collection"

=============================================================================
