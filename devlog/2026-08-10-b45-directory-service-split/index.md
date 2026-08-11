# B45 — directory inspect and commit move; derive cannot

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{directory,console,ipc,main,lib}.rs`, `components/runtime/src/syscall{,.rs}/sel4_transport.rs`, `contracts/generation/v1/fixtures/sel4-{powerbox,filesystem}.zti` |
| Roadmap | B45 |
| Gates | `just sel4_directory_check`, `just sel4_filesystem_check`, `just sel4_store_check`, `just sel4_powerbox_check`, `just sel4_dango_check` |
| Trigger | B45: directory and store service policy still reached clients through operation labels on the root endpoint. |
| Baseline | `DirectoryInspect`, `DirectoryDerive`, `DirectoryCommit` on the universal dispatcher; `StoreTransact` already removed by B43. |

## Summary

Two of the three directory labels moved to the second dispatcher with the
namespace table they mutate. The third, `DirectoryDerive`, did not, and this
entry is mostly about why: it is the only writer of the caller's `GraphTables`
entry, and the main dispatcher writes that same entry on `cap_drop` and on a
spawn's result. Moving it would have introduced a data race dressed up as a
decoupling. All five named gates pass, including two that were red before this
item for an unrelated fixture defect.

## Changes

- **New `slime-root/src/directory.rs`** holding `Namespaces`, `DisplayPath`,
  the four directory rights, `RIGHTS_DIRECTORY_ALL`, `DIRECTORY_FIXTURE_ROOT`,
  and all three handlers. Previously spread through `main.rs`, so a lib module
  could not reach them.
- **`DirectoryInspect` and `DirectoryCommit` removed** from `Operation`,
  recorded in `RETIRED_DIRECTORY_LABELS`, and served as `ConsoleKind` variants.
  `Namespaces` moved to the console thread; `ScopeTable` is read from there.
- **`RIGHT_TRANSFER` now comes from `boot-contracts`**, its canonical
  definition, instead of being restated in the binary next to a comment saying
  it was restated.
- **`ConsoleTables`** replaces nine positional references in
  `start_console_dispatcher`'s signature.
- **`sel4-powerbox.zti` and `sel4-filesystem.zti`** declare their minted run
  tokens.

## Regression guards

- `ipc::tests::no_console_operation_is_reachable_on_the_universal_abi` covers
  the two new holes through `RETIRED_DIRECTORY_LABELS`.
- `check-sel4-component-graph.py` counts retired labels and fails if any
  resolves in `from_label` again.
- `just sel4_powerbox_check` and `just sel4_filesystem_check` now pass, so both
  planes' run tokens are asserted rather than merely present.

## Verification

| Check | Result |
|---|---|
| `just sel4_directory_check` | pass — 17 markers, 2 commits, both refusal arms |
| `just sel4_filesystem_check` | pass (was red before this item) |
| `just sel4_store_check` | pass |
| `just sel4_powerbox_check` | pass (was red before this item) |
| `just sel4_dango_check` | pass |
| Fifteen further plane gates | pass |
| `cargo test -p slime-root --lib` | 143 passed |
| `just contracts_check`, `just generation_check`, `just test_host` | pass |
| `just lint_all`, `just fmt_check_all`, `just typos`, `just machete` | clean |

## Decisions

**`DirectoryDerive` stays on the main dispatcher.** It resolves the caller's
capability, interns a narrower scope, and installs the derived capability into
the caller's `GraphTables` entry. That entry is also written by the main loop —
`cap_drop` at `main.rs`, the spawn result path, reclamation. Two threads
writing one task's capability table is a data race. The alternatives were a
lock (which the root has nowhere else, and which would make the second
dispatcher block on the first — the exact coupling B41 removed) or moving
`GraphTables` wholesale (which is B50's item, since the global authority
database is what forces the choice). Splitting the three handlers is the honest
answer: two of them have an owner, one does not yet.

**`ScopeTable` is shared read-only rather than moved.** Derive is its only
writer and stays behind. A scope, once interned, is never mutated or freed, so
the console thread reading a path while derive appends a new entry observes
either the old table or the new one and never a torn scope. This is weaker than
the device tables' exclusive ownership and is called out as such.

**The rights constants moved with the handlers, not with the other rights.**
`main.rs` holds a dozen `RIGHT_*` constants for other resources. Splitting the
directory four out looks inconsistent until you notice the module is now their
only consumer; leaving them behind would have meant four `pub(crate)` exports
for a module that owns the concept.

### Two gates were red on arrival

`sel4_powerbox_check` had been red since the start of this backlog run and
`sel4_filesystem_check` alongside it. Neither is a B45 defect: both planes have
init mint an endpoint pair and hand one end to each child as its run token,
and both fixtures declared `mintedBindings = []`. The spawn preflight expects
`parent_supplied + minted`, so it saw one requested grant against zero declared
and refused with `ungranted` before either plane ran a single scenario. This is
the third plane family with the same omission, after the probe planes and
`sel4-generation`.

## Open risks and follow-ups

- The `ScopeTable` sharing argument depends on scopes never being freed. That
  is true today and is not asserted anywhere; if scope reclamation is ever
  added, this becomes a use-after-free rather than a stale read.
- The second dispatcher now serves five kinds across three unrelated concerns
  and is still called `console`. B43's entry flagged the naming; with directory
  traffic on it the name is actively misleading.
- Nine retired labels. The three constants plus two arrays that hold them want
  consolidating into one table.

## Artifacts and provenance

- Commit: `889fd7e`.
- The "red before this item" claims were verified by stashing the work and
  re-running both gates, not inferred.
