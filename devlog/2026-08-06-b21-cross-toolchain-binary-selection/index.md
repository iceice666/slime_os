# B21 — the toolchain was pinned by name, so each host resolved a different binary

| Field | Value |
|---|---|
| Date | 2026-08-06 |
| Kind | Defect |
| Status | Verified |
| Scope | `flake.nix`, `scripts/build/build-sel4.py`, `scripts/check/check-sel4-pins.py`, `sel4/pins.toml` |
| Roadmap | B21, B20, B19 |
| Gates | `just sel4_pin_check`, `just sel4_qemu_image_check` |
| Trigger | Review of B20's open follow-up: "the fix neutralizes today's wrapper difference rather than making the wrappers identical" |
| Baseline | B20 (`63221ed`): `kernel_sha256` `97dcb029…` on three systems, attributed to a per-platform `cc-cflags-before` difference |

## Summary

B20 recorded that `aarch64-darwin`'s cross `gcc-wrapper` injects
`-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer` through
`nix-support/cc-cflags-before` while `aarch64-linux`'s native `gcc` "forces
neither", and neutralized the difference by stating the opposite flags on the
command line. **That root cause is wrong.** Both systems' wrappers ship a
byte-identical `cc-cflags-before`. The actual divergence was never about wrapper
*policy*; it was about which binary ran. `CROSS_COMPILER_PREFIX` was a bare
`aarch64-unknown-linux-gnu-`, resolved against `PATH`. On `aarch64-linux`
nixpkgs' `pkgsCross.aarch64-multiplatform.stdenv.cc` is a *native* wrapper whose
`bin/` contains no `aarch64-unknown-linux-gnu-`-prefixed entry at all, so the
lookup fell through it to the **unwrapped** GCC — a different compiler driver
*and* a different assembler. Fixed by exporting `CROSS_COMPILER_PREFIX` as an
absolute store path, so every host runs the same binaries. B20's flags are kept:
fault injection showed they close a *separate*, previously unrecorded residual
leak in `.debug_line`.

## Observable symptom

The pinned hash agreed, so the defect was latent rather than gate-visible. It
surfaces as soon as the flags B20 added are removed — that is, B20's own
regression guard was passing for a reason different from the one recorded.

- Command: in the pinned shell on each host, `command -v aarch64-unknown-linux-gnu-gcc`.
- Expected: the same wrapper derivation, differing only in system.
- Observed: `aarch64-darwin` resolves
  `/nix/store/vwv7j4…-aarch64-unknown-linux-gnu-gcc-wrapper-15.2.0/bin/aarch64-unknown-linux-gnu-gcc`;
  `aarch64-linux` resolves
  `/nix/store/8f6sbb…-gcc-15.2.0/bin/aarch64-unknown-linux-gnu-gcc` — the
  **unwrapped** compiler, reached past the wrapper that shadows `gcc` but
  publishes no prefixed name.
- Evidence: `CROSS_COMPILER_PREFIX` was empty on `aarch64-linux`
  (`crossCC.targetPrefix` of a native wrapper), so `build-sel4.py` fell back to
  its hardcoded `aarch64-unknown-linux-gnu-` default and searched `PATH`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Read `nix-support/cc-cflags-before` from both realized wrappers. Darwin: `-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer -march=armv8-a`. `aarch64-linux`: **the same string**. | B20's stated cause is false. nixpkgs gates this on `targetPlatform`, which is `aarch64` for both; `cc-wrapper/default.nix:903` emits it whenever `!isx86_32 && !isS390`. |
| 2 | `pkgsCross.aarch64-multiplatform.stdenv.cc.targetPrefix` is `aarch64-unknown-linux-gnu-` on Darwin and `x86_64-linux`, but `""` on `aarch64-linux` (native wrapper, `hostPlatform == targetPlatform`). | `flake.nix` exported an *empty* `CROSS_COMPILER_PREFIX` there, so the build used its hardcoded default. |
| 3 | Listed the native wrapper's `bin/`: 23 entries, none prefixed with `aarch64-unknown-linux-gnu-`. | The prefixed name cannot be satisfied by the wrapper, so `PATH` search continues to the next entry. |
| 4 | In the shell on `aarch64-linux`, the prefixed `gcc` resolves into the **unwrapped** `gcc-15.2.0`, which is second on `PATH` via the wrapper's `setup-hook`. Bare `gcc` resolves to the wrapper. | Two different compiler drivers were in use across hosts, which is a superset of a flag difference. |
| 5 | Leaf function at `-O2`: wrapper emits the `stp x29, x30` prologue, unwrapped emits bare `add w0, w0, 1; ret`. Same on both hosts for the same binary. | The frame-pointer delta B20 measured is real, but it tracks *wrapped vs unwrapped*, not *Darwin vs Linux*. |
| 6 | Built the kernel on Darwin forcing the **unwrapped** compiler: `97dcb029…` — identical to the pinned value. Forcing the **absolute wrapper** path: also `97dcb029…`. | With B20's flags present, both driver choices converge, which is why the gate passed and hid the defect. |
| 7 | Fault injection (flags removed) on Darwin: unwrapped → `da8bbaa4…`, wrapped → `e8cbab4f…`. On `aarch64-linux`, unwrapped → `f2d316e1…`. | `e8cbab4f…` and `f2d316e1…` are exactly B20's recorded "Darwin" and "Linux" pre-fix hashes — confirming those two numbers differed by **driver**, not by host. |
| 8 | Compiled `kernel_all.c` to assembly on `aarch64-linux`, then assembled that *one* `.s` with each host's `as`, controlling for cwd and output filename. Darwin: `10dfe174…` (1840600 bytes). Linux: `c1d8cb43…` (1840640). | The **assemblers** disagree too, independent of the compiler. A compiler-flag fix could never have reached this. |
| 9 | Per-section comparison of those objects: every section matches except `.debug_line`. Net opcode delta is one row — Linux emits `set Address` + `Copy` where Darwin emits `Special opcode 61`. Decoded line tables (`--dwarf=decodedline`) are **identical**. | A DWARF-5 *view*-numbering difference in GAS, cosmetic to debuggers but byte-visible to a SHA-256 pin. |
| 10 | With the toolchain pinned absolutely but B20's flags removed, the two hosts still differ (`e8cbab4f…` vs `4c694979…`, both 982208 bytes); every ALLOC section matches and only `.debug_line` differs. | B20's flags remain load-bearing — for step 9's reason, not the recorded one. Omitting the frame pointer removes the extra line-table row that triggers the divergence. |

## Root cause

`flake.nix` pinned the toolchain by *name* — `CROSS_COMPILER_PREFIX =
crossCC.targetPrefix` — and `build-sel4.py` passed that bare prefix to CMake,
which resolves `${CROSS_COMPILER_PREFIX}gcc` through `PATH`. A name is not an
identity: nixpkgs' `pkgsCross.aarch64-multiplatform.stdenv.cc` is a *cross*
wrapper on `aarch64-darwin` and `x86_64-linux` but a *native* wrapper on
`aarch64-linux`, where `targetPrefix` is empty and no prefixed binary is
published. The prefixed lookup therefore skipped the wrapper entirely and found
the unwrapped GCC that the wrapper's own `setup-hook` had added to `PATH`.

The violated invariant is the one `[observed_prefix]` asserts: the pinned hashes
are a function of the pinned inputs. The compiler driver and the assembler were
unpinned inputs, selected by `PATH` order.

This is the same class of error B19 and B20 each fixed one layer of — B19 removed
the *environment*'s influence, B20 removed one *flag*'s influence — but neither
pinned the *binary*. B20 went further and misattributed the residue to a wrapper
policy difference that does not exist, which is why its follow-up predicted the
wrong future failure ("a nixpkgs change adding a different `cc-cflags-before`
entry"): the wrappers were never the asymmetric part.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `flake.nix` | `CROSS_COMPILER_PREFIX` is now `"${crossCC}/bin/${crossCC.targetPrefix}"` — an absolute path into the wrapper's own `bin/`. The `crossCC` comment records that the attribute's *shape* differs per system while its injected flags do not. | The compiler driver and assembler are pinned by store path, not by `PATH` order. |
| `scripts/check/check-sel4-pins.py` | New assertion in `check_toolchain_and_targets`: `flake.nix` must export the absolute form. | Reverting to a bare prefix fails a cheap static gate instead of silently changing `kernel.elf`. |
| `scripts/build/build-sel4.py` | `cross_compiler_prefix` documents why the shell passes an absolute path and why a bare prefix is still accepted for hosts outside the pinned shell. The fourth reproducibility bullet is corrected: it no longer claims the wrappers differ, and it records the `.debug_line` evidence that keeps the flags. | Prose matches observed behavior. |
| `sel4/pins.toml` | Provenance note corrected: names absolute-path pinning as a fourth mechanism, and states that all three wrappers ship the same `cc-cflags-before`. | The recorded reason for the pin's platform independence is the true one. |

`kernel_sha256` is **unchanged** at `97dcb029…`. This fix moves no pinned hash;
it makes the existing one depend on the toolchain rather than on `PATH`.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| `CROSS_COMPILER_PREFIX` reverts to a bare triple prefix | `just sel4_pin_check` | Fault-injected: reverting to `crossCC.targetPrefix` fails with "flake.nix must export CROSS_COMPILER_PREFIX as an absolute …" |
| A host resolves a different compiler or assembler | `just sel4_qemu_image_check` | `kernel.elf` SHA-256 mismatch against `[observed_prefix]` |
| B20's frame-pointer flags are dropped | `just sel4_qemu_image_check` | Fault-injected: `e8cbab4f…` on `aarch64-darwin`, `4c694979…` on `aarch64-linux` — still divergent, now only in `.debug_line` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| **Exit condition:** `kernel.elf` rebuilt from scratch on `aarch64-darwin` and `aarch64-linux` with the absolute prefix | Both `97dcb029127bcc0fac1fd6cc83950ba878040a773a380e2d000b0ebc20d875ce`, 973184 bytes — unchanged from the recorded pin | Direct |
| `CROSS_COMPILER_PREFIX` observed inside the shell on `aarch64-linux` | `/nix/store/d6gsw7k…-gcc-wrapper-15.2.0/bin/` — the wrapper, no longer empty | Direct |
| `just sel4_qemu_image_check` on `aarch64-darwin` | Pass — includes the `--prefix` pin check verifying `97dcb029…` | Direct |
| `just sel4_pin_check`, `just ruff`, `just typos` | Pass | Direct |
| Guard fault injection: `CROSS_COMPILER_PREFIX` reverted to `crossCC.targetPrefix` | `just sel4_pin_check` fails with the B21 message; restored afterward | Direct |
| Both wrappers' `nix-support/cc-cflags-before` compared | Byte-identical on `aarch64-darwin` and `aarch64-linux`, disproving B20's stated cause | Direct |
| Single `.s`, each host's `as`, cwd and output name controlled | `10dfe174…` vs `c1d8cb43…`; all sections equal except `.debug_line`; decoded line tables identical | Direct |
| Flags-removed fault injection with the toolchain pinned | `e8cbab4f…` vs `4c694979…`, both 982208 bytes, differing only in `.debug_line` | Direct |
| `x86_64-linux` | **Not re-observed for this change.** Its `targetPrefix` is already the cross form, so it resolved the wrapper before and after; the absolute path is expected to be a no-op there. **[INFERENCE]** | Inferred |

`aarch64-linux` is OrbStack's Docker engine running `nixos/nix:latest` over this
checkout and the same `flake.nix`, on a persistent `/nix` volume. It is a Linux
kernel under a macOS hypervisor, not separate hardware — the right test for
toolchain and `PATH` independence, and no evidence about physical boards.

## Decisions

- Decision: pin the toolchain by absolute store path rather than by triple name.
- Rationale: this is the fix B20 proposed, rejected, and left as "optional… the
  stronger fix". The rejection rested on a false premise — that it "would make
  the Darwin and Linux shells fetch a toolchain neither platform's nixpkgs
  selects by default" and "moves the recorded hash". Neither holds for the form
  used here: `crossCC` is still exactly `pkgsCross.aarch64-multiplatform.stdenv.cc`,
  the same derivation each platform already evaluates and already installs into
  the shell. Only the *reference* changes, from a `PATH`-resolved name to the
  store path of that same package. Nothing new is fetched and `kernel_sha256`
  does not move — both verified.
- Decision: keep B20's `-fomit-frame-pointer -momit-leaf-frame-pointer`.
- Rationale: they no longer serve the purpose B20 recorded, but fault injection
  shows they close a real, separate leak — GAS's DWARF-5 view numbering for the
  extra prologue row is not host-independent. Removing them reintroduces
  divergence even with the toolchain pinned. They also remain a defensible codegen
  policy on their own terms, and stating them means a future `cc-cflags-before`
  change cannot move the pin through that flag.
- Rejected alternative: naming `pkgsCross.aarch64-embedded` (`aarch64-none-elf-`),
  a genuine cross toolchain on all three systems. It is uniform, but it *is* the
  change B20 feared: a different libc-less GCC that no platform selects by
  default, requiring a fetch and moving every pinned hash. Verified it ships the
  same `cc-cflags-before` and that its assembler reproduces the same host split
  (`10dfe174…` on Darwin, `c1d8cb43…` on Linux), so it would not even fix the
  assembler difference. Rejected.
- Rejected alternative: scrubbing the wrapper's injection via
  `NIX_CC_WRAPPER_FLAGS_SET_<salt>=1`. Verified it does suppress
  `cc-cflags-before`, but it relies on an internal nixpkgs variable and disables
  every injection at once, including the libc paths. Too blunt and too coupled.

## Open risks and follow-ups

- [ ] The `.debug_line` view-numbering difference between the two hosts' `as` is
      **masked, not fixed**. Today no line-table row exists at the address that
      triggers it; a future seL4 or GCC change could reintroduce one, and the gate
      would report toolchain drift. The underlying binutils behavior is worth an
      upstream report. Reproduction is recorded in step 8 above.
- [ ] `x86_64-linux` was not re-observed for this change; the no-op claim there is
      **[INFERENCE]**, not measurement.
- [ ] `[observed_prefix]` still binds build tooling `sel4/pins.toml` does not pin
      — `cmake`, `ninja`, and the host Python generators. Carried forward from B19
      and B20 unchanged.
- [ ] Still unobserved: a second *physical* machine. Both hosts here are on one
      machine, one of them virtualized.

## Artifacts and provenance

- Backlog entry: `roadmap/00-backlog.md`, resolved log, B21.
- Predecessor: `devlog/2026-08-06-b20-cross-platform-kernel-identity/` — whose
  root cause this corrects and whose open follow-up this closes. That entry's
  body is frozen; the correction is recorded in its `## Corrections` section.
- Earlier predecessor: `devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/`.
- Related roadmap item: `roadmap/07-architecture-portability.md` — P5, whose
  `[observed_prefix]` gate this makes a genuine toolchain gate.
