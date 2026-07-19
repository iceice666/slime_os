------------------------------- MODULE BootState -------------------------------
EXTENDS Integers, FiniteSets, Sequences, TLC

CONSTANTS Generations, NoGeneration, NoGraphRoot,
          MaxAttempts, MaxSequence, MaxEpoch, RequiredCut

ASSUME /\ NoGeneration \notin Generations
       /\ MaxEpoch >= 1

SlotNames == {"A", "B"}
StatePolicies == {"immutable", "ephemeral", "preserve",
                  "snapshotBeforeUpgrade", "discardOnRollback"}
PolicyOrder == <<"immutable", "ephemeral", "preserve",
                 "snapshotBeforeUpgrade", "discardOnRollback">>

GraphRoot(generation, epoch) == <<"graph", generation, epoch>>
GraphRoots == {GraphRoot(generation, epoch) :
               generation \in Generations, epoch \in 0..MaxEpoch}
RootGeneration(root) == root[2]
RootEpoch(root) == root[3]

PersistentJournal == <<"persistent-state", "journal">>

StateObject(policy, root) ==
    CASE policy = "immutable" -> <<"state", policy, "shared">>
      [] policy = "preserve" -> <<"state", policy, "shared">>
      [] OTHER -> <<"state", policy, RootGeneration(root), RootEpoch(root)>>

CanonicalBindings(root) ==
    {StateObject(policy, root) : policy \in StatePolicies}

StateObjectUniverse ==
    {PersistentJournal} \cup UNION {CanonicalBindings(root) : root \in GraphRoots}

SnapshotObject(policy, targetRoot, baseRoot) ==
    CASE policy = "immutable" -> StateObject(policy, baseRoot)
      [] policy = "preserve" -> StateObject(policy, baseRoot)
      [] policy = "ephemeral" -> StateObject(policy, targetRoot)
      [] policy = "snapshotBeforeUpgrade" -> StateObject(policy, targetRoot)
      [] policy = "discardOnRollback" -> StateObject(policy, targetRoot)

SnapshotObjectForCommit(policy, targetRoot, baseRoot) ==
    SnapshotObject(policy, targetRoot, baseRoot)

MixedEpochSnapshotObject(policy, targetRoot, baseRoot) ==
    IF policy = "snapshotBeforeUpgrade"
    THEN StateObject(policy, baseRoot)
    ELSE IF policy = "ephemeral"
    THEN PersistentJournal
    ELSE SnapshotObject(policy, targetRoot, baseRoot)

SnapshotBindings(targetRoot, baseRoot) ==
    {SnapshotObjectForCommit(policy, targetRoot, baseRoot) :
     policy \in StatePolicies}

EmptyRecord == [
    valid |-> FALSE,
    sequence |-> 0,
    knownGood |-> NoGeneration,
    knownGoodRoot |-> NoGraphRoot,
    pending |-> NoGeneration,
    pendingRoot |-> NoGraphRoot,
    remainingAttempts |-> 0
]

RecordType == [
    valid : BOOLEAN,
    sequence : 0..MaxSequence,
    knownGood : Generations \cup {NoGeneration},
    knownGoodRoot : GraphRoots \cup {NoGraphRoot},
    pending : Generations \cup {NoGeneration},
    pendingRoot : GraphRoots \cup {NoGraphRoot},
    remainingAttempts : 0..MaxAttempts
]

RecordConsistent(record) ==
    /\ record \in RecordType
    /\ record.knownGood \in Generations
    /\ record.knownGoodRoot \in GraphRoots
    /\ RootGeneration(record.knownGoodRoot) = record.knownGood
    /\ record.pending \in Generations \cup {NoGeneration}
    /\ IF record.pending = NoGeneration
       THEN /\ record.pendingRoot = NoGraphRoot
            /\ record.remainingAttempts = 0
       ELSE /\ record.pendingRoot \in GraphRoots
            /\ RootGeneration(record.pendingRoot) = record.pending
            /\ record.remainingAttempts \in 0..MaxAttempts

VARIABLES slots,
          durableGenerations, durableGraphRoots, durableStateObjects,
          graphBindings, rollbackRoots, persistentStateRoots,
          phase, running, runningRoot, runningPending,
          writeTarget, writeValue, writeKind, writeStage,
          attemptsBeforeTransfer, lastCut,
          stagedCandidate, stagedRoot, snapshotBaseRoot, snapshotProgress,
          gcCandidate

vars == <<slots,
          durableGenerations, durableGraphRoots, durableStateObjects,
          graphBindings, rollbackRoots, persistentStateRoots,
          phase, running, runningRoot, runningPending,
          writeTarget, writeValue, writeKind, writeStage,
          attemptsBeforeTransfer, lastCut,
          stagedCandidate, stagedRoot, snapshotBaseRoot, snapshotProgress,
          gcCandidate>>

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

GraphComplete(root) ==
    /\ root \in durableGraphRoots
    /\ graphBindings[root] = CanonicalBindings(root)
    /\ graphBindings[root] \subseteq durableStateObjects

BootableRecord(record) ==
    /\ record.valid
    /\ RecordConsistent(record)
    /\ record.knownGood \in durableGenerations
    /\ GraphComplete(record.knownGoodRoot)
    /\ record.pending # NoGeneration =>
          /\ record.pending \in durableGenerations
          /\ GraphComplete(record.pendingRoot)

ValidSlots == {slot \in SlotNames : slots[slot].valid}
ValidGenerationRoots ==
    {slots[slot].knownGood : slot \in ValidSlots}
    \cup {slots[slot].pending : slot \in
             {candidate \in ValidSlots : slots[candidate].pending # NoGeneration}}
ValidGraphRoots ==
    {slots[slot].knownGoodRoot : slot \in ValidSlots}
    \cup {slots[slot].pendingRoot : slot \in
             {candidate \in ValidSlots : slots[candidate].pending # NoGeneration}}
RunningGraphRoots == IF runningRoot = NoGraphRoot THEN {} ELSE {runningRoot}
StagedGraphRoots == IF stagedRoot = NoGraphRoot THEN {} ELSE {stagedRoot}
RetainedGraphRoots == ValidGraphRoots \cup RunningGraphRoots \cup rollbackRoots
StagedTransactionRoots == StagedGraphRoots
CheckedGraphRoots ==
    RetainedGraphRoots \cup (StagedTransactionRoots \cap durableGraphRoots)
RetainedGenerations ==
    ValidGenerationRoots
    \cup {RootGeneration(root) : root \in rollbackRoots}
    \cup (IF running = NoGeneration THEN {} ELSE {running})
    \cup (IF stagedCandidate = NoGeneration THEN {} ELSE {stagedCandidate})
RetainedStateObjects ==
    persistentStateRoots
    \cup UNION {graphBindings[root] : root \in RetainedGraphRoots}
    \cup UNION {CanonicalBindings(root) \cap durableStateObjects :
                root \in StagedTransactionRoots}
ProtectedIdentities ==
    RetainedGenerations \cup RetainedGraphRoots \cup StagedTransactionRoots
    \cup RetainedStateObjects

GcProtectedIdentities == ProtectedIdentities
OmitPersistentRootProtection == ProtectedIdentities \ persistentStateRoots

DurableIdentities ==
    durableGenerations \cup durableGraphRoots \cup durableStateObjects

ChooseGcCandidate(candidates) == CHOOSE candidate \in candidates : TRUE
ChoosePersistentCandidate(candidates) ==
    IF PersistentJournal \in candidates
    THEN PersistentJournal
    ELSE ChooseGcCandidate(candidates)

TypeOK ==
    /\ slots \in [SlotNames -> RecordType]
    /\ durableGenerations \subseteq Generations
    /\ durableGraphRoots \subseteq GraphRoots
    /\ durableStateObjects \subseteq StateObjectUniverse
    /\ graphBindings \in [GraphRoots -> SUBSET StateObjectUniverse]
    /\ rollbackRoots \subseteq GraphRoots
    /\ persistentStateRoots \subseteq StateObjectUniverse
    /\ phase \in {"boot", "running", "attempt-committed", "pending-committed",
                    "snapshot", "gc"}
    /\ running \in Generations \cup {NoGeneration}
    /\ runningRoot \in GraphRoots \cup {NoGraphRoot}
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
    /\ stagedRoot \in GraphRoots \cup {NoGraphRoot}
    /\ snapshotBaseRoot \in GraphRoots \cup {NoGraphRoot}
    /\ snapshotProgress \in 0..Len(PolicyOrder)
    /\ gcCandidate \in Generations \cup GraphRoots \cup StateObjectUniverse \cup {"none"}

Init ==
    LET initialGeneration == CHOOSE generation \in Generations : TRUE
        initialRoot == GraphRoot(initialGeneration, 0)
        initialBindings == CanonicalBindings(initialRoot)
        initial == [
            valid |-> TRUE,
            sequence |-> 1,
            knownGood |-> initialGeneration,
            knownGoodRoot |-> initialRoot,
            pending |-> NoGeneration,
            pendingRoot |-> NoGraphRoot,
            remainingAttempts |-> 0
        ]
    IN /\ Cardinality(Generations) = 2
       /\ MaxAttempts = 2
       /\ MaxSequence = 5
       /\ MaxEpoch = 1
       /\ slots = [slot \in SlotNames |->
                       IF slot = "A" THEN [initial EXCEPT !.sequence = 2] ELSE initial]
       /\ durableGenerations = Generations
       /\ durableGraphRoots = {initialRoot}
       /\ durableStateObjects = initialBindings \cup {PersistentJournal}
       /\ graphBindings = [root \in GraphRoots |->
                                IF root = initialRoot THEN initialBindings ELSE {}]
       /\ rollbackRoots = {}
       /\ persistentStateRoots = {PersistentJournal}
       /\ phase = "boot"
       /\ running = NoGeneration
       /\ runningRoot = NoGraphRoot
       /\ runningPending = FALSE
       /\ writeTarget = "none"
       /\ writeValue = EmptyRecord
       /\ writeKind = "none"
       /\ writeStage = "idle"
       /\ attemptsBeforeTransfer = 0
       /\ lastCut = "none"
       /\ stagedCandidate = NoGeneration
       /\ stagedRoot = NoGraphRoot
       /\ snapshotBaseRoot = NoGraphRoot
       /\ snapshotProgress = 0
       /\ gcCandidate = "none"

BootKnownGood ==
    /\ phase = "boot"
    /\ writeStage = "idle"
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)
    /\ SelectedState.pending = NoGeneration
    /\ phase' = "running"
    /\ running' = SelectedState.knownGood
    /\ runningRoot' = SelectedState.knownGoodRoot
    /\ runningPending' = FALSE
    /\ attemptsBeforeTransfer' = 0
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, writeTarget, writeValue, writeKind,
                    writeStage, lastCut, stagedCandidate, stagedRoot,
                    snapshotBaseRoot, snapshotProgress, gcCandidate>>

BootExhaustedKnownGood ==
    /\ phase = "boot"
    /\ writeStage = "idle"
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)
    /\ SelectedState.pending # NoGeneration
    /\ SelectedState.remainingAttempts = 0
    /\ phase' = "running"
    /\ running' = SelectedState.knownGood
    /\ runningRoot' = SelectedState.knownGoodRoot
    /\ runningPending' = FALSE
    /\ attemptsBeforeTransfer' = 0
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, writeTarget, writeValue, writeKind,
                    writeStage, lastCut, stagedCandidate, stagedRoot,
                    snapshotBaseRoot, snapshotProgress, gcCandidate>>

BeginStagePending ==
    /\ phase = "running"
    /\ runningPending = FALSE
    /\ SelectedSlot \in SlotNames
    /\ running = SelectedState.knownGood
    /\ runningRoot = SelectedState.knownGoodRoot
    /\ SelectedState.pending = NoGeneration
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ stagedCandidate = NoGeneration
    /\ \E candidate \in durableGenerations \ {SelectedState.knownGood}:
          stagedCandidate' = candidate
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut, stagedRoot,
                    snapshotBaseRoot, snapshotProgress, gcCandidate>>

BeginUpgradeSnapshot ==
    /\ phase = "running"
    /\ stagedCandidate \in durableGenerations
    /\ stagedRoot = NoGraphRoot
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.knownGoodRoot \in GraphRoots
    /\ RootEpoch(SelectedState.knownGoodRoot) < MaxEpoch
    /\ stagedRoot' = GraphRoot(stagedCandidate,
                                RootEpoch(SelectedState.knownGoodRoot) + 1)
    /\ stagedRoot' \notin durableGraphRoots
    /\ snapshotBaseRoot' = SelectedState.knownGoodRoot
    /\ snapshotProgress' = 0
    /\ phase' = "snapshot"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, running, runningRoot, runningPending,
                    writeTarget, writeValue, writeKind, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate, gcCandidate>>

WriteSnapshotPolicy ==
    /\ phase = "snapshot"
    /\ stagedRoot \in GraphRoots
    /\ snapshotBaseRoot \in durableGraphRoots
    /\ snapshotProgress < Len(PolicyOrder)
    /\ LET policy == PolicyOrder[snapshotProgress + 1]
           object == SnapshotObjectForCommit(policy, stagedRoot, snapshotBaseRoot)
       IN durableStateObjects' = durableStateObjects \cup {object}
    /\ snapshotProgress' = snapshotProgress + 1
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    graphBindings, rollbackRoots, persistentStateRoots,
                    phase, running, runningRoot, runningPending, writeTarget,
                    writeValue, writeKind, writeStage, attemptsBeforeTransfer,
                    lastCut, stagedCandidate, stagedRoot, snapshotBaseRoot,
                    gcCandidate>>

SealSnapshot ==
    /\ phase = "snapshot"
    /\ stagedRoot \in GraphRoots
    /\ snapshotBaseRoot \in durableGraphRoots
    /\ snapshotProgress = Len(PolicyOrder)
    /\ SnapshotBindings(stagedRoot, snapshotBaseRoot) \subseteq durableStateObjects
    /\ graphBindings' = [graphBindings EXCEPT
                              ![stagedRoot] = SnapshotBindings(stagedRoot, snapshotBaseRoot)]
    /\ durableGraphRoots' = durableGraphRoots \cup {stagedRoot}
    /\ snapshotBaseRoot' = NoGraphRoot
    /\ snapshotProgress' = 0
    /\ phase' = "running"
    /\ UNCHANGED <<slots, durableGenerations, durableStateObjects,
                    rollbackRoots, persistentStateRoots, running, runningRoot,
                    runningPending, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut,
                    stagedCandidate, stagedRoot, gcCandidate>>

StartPendingMetadata ==
    /\ phase = "running"
    /\ runningPending = FALSE
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.pending = NoGeneration
    /\ stagedCandidate \in durableGenerations
    /\ stagedRoot \in durableGraphRoots
    /\ RootGeneration(stagedRoot) = stagedCandidate
    /\ GraphComplete(stagedRoot)
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ writeValue' = [SelectedState EXCEPT
                        !.pending = stagedCandidate,
                        !.pendingRoot = stagedRoot,
                        !.remainingAttempts = MaxAttempts]
    /\ writeKind' = "stage-pending"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress, gcCandidate>>

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
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeStage, lastCut,
                    stagedCandidate, stagedRoot, snapshotBaseRoot,
                    snapshotProgress, gcCandidate>>

BeginHealthPromotion ==
    /\ phase = "running"
    /\ runningPending = TRUE
    /\ SelectedSlot \in SlotNames
    /\ running = SelectedState.pending
    /\ runningRoot = SelectedState.pendingRoot
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ writeValue' = [SelectedState EXCEPT
                        !.knownGood = running,
                        !.knownGoodRoot = runningRoot,
                        !.pending = NoGeneration,
                        !.pendingRoot = NoGraphRoot,
                        !.remainingAttempts = 0]
    /\ writeKind' = "promotion"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress, gcCandidate>>

BeginRollback ==
    /\ phase = "running"
    /\ SelectedSlot \in SlotNames
    /\ \/ runningPending
       \/ /\ ~runningPending
          /\ running = SelectedState.knownGood
          /\ runningRoot = SelectedState.knownGoodRoot
          /\ SelectedState.pending # NoGeneration
          /\ SelectedState.remainingAttempts = 0
    /\ writeKind = "none"
    /\ writeStage = "idle"
    /\ writeValue' = [SelectedState EXCEPT
                        !.pending = NoGeneration,
                        !.pendingRoot = NoGraphRoot,
                        !.remainingAttempts = 0]
    /\ writeKind' = "rollback"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress, gcCandidate>>

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
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, attemptsBeforeTransfer, lastCut,
                    stagedCandidate, stagedRoot, snapshotBaseRoot,
                    snapshotProgress, gcCandidate>>

PrepareWriteStep == PrepareWrite(writeKind, writeValue)

InvalidateWriteTarget ==
    /\ writeStage = "prepared"
    /\ writeTarget \in SlotNames
    /\ slots' = [slots EXCEPT ![writeTarget].valid = FALSE]
    /\ writeStage' = "invalidated"
    /\ UNCHANGED <<durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeValue, writeKind,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress, gcCandidate>>

WritePayload ==
    /\ writeStage = "invalidated"
    /\ writeTarget \in SlotNames
    /\ slots' = [slots EXCEPT ![writeTarget] = [writeValue EXCEPT !.valid = FALSE]]
    /\ writeStage' = "payload-written"
    /\ UNCHANGED <<durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeValue, writeKind,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress, gcCandidate>>

FlushPayload ==
    /\ writeStage = "payload-written"
    /\ writeStage' = "flushed"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, phase, running, runningRoot,
                    runningPending, writeTarget, writeValue, writeKind,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress, gcCandidate>>

CommitWrite ==
    /\ writeStage = "flushed"
    /\ writeTarget \in SlotNames
    /\ SelectedSlot = OtherSlot(writeTarget)
    /\ slots' = [slots EXCEPT
                   ![writeTarget] = [writeValue EXCEPT
                                      !.sequence = SelectedState.sequence + 1,
                                      !.valid = TRUE]]
    /\ rollbackRoots' = IF writeKind = "promotion"
                         THEN rollbackRoots \cup {SelectedState.knownGoodRoot}
                         ELSE rollbackRoots
    /\ phase' = CASE writeKind = "stage-pending" -> "pending-committed"
                  [] writeKind = "consume-attempt" -> "attempt-committed"
                  [] writeKind = "promotion" -> "running"
                  [] writeKind = "rollback" -> "boot"
    /\ running' = IF writeKind = "rollback" THEN NoGeneration ELSE running
    /\ runningRoot' = IF writeKind = "rollback" THEN NoGraphRoot ELSE runningRoot
    /\ runningPending' = IF writeKind \in {"promotion", "rollback"}
                          THEN FALSE ELSE runningPending
    /\ stagedCandidate' = IF writeKind = "stage-pending"
                           THEN NoGeneration ELSE stagedCandidate
    /\ stagedRoot' = IF writeKind = "stage-pending"
                      THEN NoGraphRoot ELSE stagedRoot
    /\ writeTarget' = "none"
    /\ writeValue' = EmptyRecord
    /\ writeKind' = "none"
    /\ writeStage' = "idle"
    /\ UNCHANGED <<durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, persistentStateRoots,
                    attemptsBeforeTransfer, lastCut, snapshotBaseRoot,
                    snapshotProgress, gcCandidate>>

RebootAfterPendingCommit ==
    /\ phase = "pending-committed"
    /\ writeStage = "idle"
    /\ phase' = "boot"
    /\ running' = NoGeneration
    /\ runningRoot' = NoGraphRoot
    /\ runningPending' = FALSE
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut,
                    stagedCandidate, stagedRoot, snapshotBaseRoot,
                    snapshotProgress, gcCandidate>>

TransferPending ==
    /\ phase = "attempt-committed"
    /\ writeStage = "idle"
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.pending # NoGeneration
    /\ phase' = "running"
    /\ running' = SelectedState.pending
    /\ runningRoot' = SelectedState.pendingRoot
    /\ runningPending' = TRUE
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, writeTarget, writeValue, writeKind,
                    writeStage, attemptsBeforeTransfer, lastCut,
                    stagedCandidate, stagedRoot, snapshotBaseRoot,
                    snapshotProgress, gcCandidate>>

BeginGc ==
    /\ phase = "running"
    /\ gcCandidate = "none"
    /\ LET candidates == DurableIdentities \ GcProtectedIdentities
       IN /\ candidates # {}
          /\ gcCandidate' = ChooseGcCandidate(candidates)
    /\ phase' = "gc"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, running, runningRoot, runningPending,
                    writeTarget, writeValue, writeKind, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress>>

DeleteGcCandidate ==
    /\ phase = "gc"
    /\ gcCandidate # "none"
    /\ durableGenerations' = durableGenerations \ {gcCandidate}
    /\ durableGraphRoots' = durableGraphRoots \ {gcCandidate}
    /\ durableStateObjects' = durableStateObjects \ {gcCandidate}
    /\ UNCHANGED <<slots, graphBindings, rollbackRoots, persistentStateRoots,
                    phase, running, runningRoot, runningPending, writeTarget,
                    writeValue, writeKind, writeStage, attemptsBeforeTransfer,
                    lastCut, stagedCandidate, stagedRoot, snapshotBaseRoot,
                    snapshotProgress, gcCandidate>>

FinishGc ==
    /\ phase = "gc"
    /\ gcCandidate \notin DurableIdentities
    /\ gcCandidate' = "none"
    /\ phase' = "running"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots, running, runningRoot, runningPending,
                    writeTarget, writeValue, writeKind, writeStage,
                    attemptsBeforeTransfer, lastCut, stagedCandidate,
                    stagedRoot, snapshotBaseRoot, snapshotProgress>>

Crash(cut) ==
    /\ phase' = "boot"
    /\ running' = NoGeneration
    /\ runningRoot' = NoGraphRoot
    /\ runningPending' = FALSE
    /\ writeTarget' = "none"
    /\ writeValue' = EmptyRecord
    /\ writeKind' = "none"
    /\ writeStage' = "idle"
    /\ attemptsBeforeTransfer' = 0
    /\ lastCut' = cut
    /\ stagedCandidate' = NoGeneration
    /\ stagedRoot' = NoGraphRoot
    /\ snapshotBaseRoot' = NoGraphRoot
    /\ snapshotProgress' = 0
    /\ gcCandidate' = "none"
    /\ UNCHANGED <<slots, durableGenerations, durableGraphRoots,
                    durableStateObjects, graphBindings, rollbackRoots,
                    persistentStateRoots>>

CutBeforePendingAction ==
    /\ phase = "running"
    /\ stagedCandidate \in Generations
    /\ stagedRoot \in durableGraphRoots
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
    /\ stagedRoot \in GraphRoots
    /\ Crash("state-snapshot")

CutGcAction ==
    /\ phase = "gc"
    /\ gcCandidate # "none"
    /\ Crash("garbage-collection")

StutterAtSequenceBound ==
    /\ SelectedSlot \in SlotNames
    /\ SelectedState.sequence = MaxSequence
    /\ UNCHANGED vars

Next ==
    \/ BootKnownGood
    \/ BootExhaustedKnownGood
    \/ BeginStagePending
    \/ BeginUpgradeSnapshot
    \/ WriteSnapshotPolicy
    \/ SealSnapshot
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

RetainedGraphsConsistent ==
    \A root \in CheckedGraphRoots : GraphComplete(root)

SelectableBootRootExists ==
    /\ SelectedSlot \in SlotNames
    /\ BootableRecord(SelectedState)

ProtectedRootsPresent ==
    /\ RetainedGenerations \subseteq durableGenerations
    /\ RetainedGraphRoots \subseteq durableGraphRoots
    /\ RetainedStateObjects \subseteq durableStateObjects

PersistentRootsPresent ==
    persistentStateRoots \subseteq durableStateObjects

PendingAttemptConsumedBeforeTransfer ==
    ~runningPending
    \/ /\ SelectedSlot \in SlotNames
       /\ SelectedState.pending = running
       /\ SelectedState.pendingRoot = runningRoot
       /\ SelectedState.remainingAttempts = attemptsBeforeTransfer - 1

ConfirmationOnlyForRunningPending ==
    writeKind # "promotion"
    \/ /\ runningPending
       /\ running = writeValue.knownGood
       /\ runningRoot = writeValue.knownGoodRoot

WritePreservesOtherSlot ==
    writeStage = "idle"
    \/ /\ writeTarget \in SlotNames
       /\ slots[OtherSlot(writeTarget)].valid
       /\ BootableRecord(slots[OtherSlot(writeTarget)])

RunningGraphConsistent ==
    running = NoGeneration
    \/ /\ runningRoot \in GraphRoots
       /\ RootGeneration(runningRoot) = running
       /\ GraphComplete(runningRoot)

SnapshotEpochConsistent ==
    phase # "snapshot"
    \/ /\ stagedRoot \in GraphRoots
       /\ snapshotBaseRoot \in GraphRoots
       /\ RootGeneration(stagedRoot) = stagedCandidate
       /\ RootEpoch(stagedRoot) = RootEpoch(snapshotBaseRoot) + 1

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
