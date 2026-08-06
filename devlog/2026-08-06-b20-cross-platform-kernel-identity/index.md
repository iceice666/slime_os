# B20 — the prefix pin held for one platform at a time

| Field | Value |
|---|---|
| Date | 2026-08-06 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/build/build-sel4.py`, `sel4/pins.toml`, `flake.nix` |
| Roadmap | B20, B19 |
| Gates | `just sel4_qemu_image_check` |
| Trigger | B19's second-host test (`9637555`) observed `f2d316e1…` on `aarch64-linux` against `e8cbab4f…` on `aarch64-darwin` |
| Baseline | B19 (`dad310a`): `kernel_sha256` independent of the dev shell, but observed on one platform only |

## Summary

B19 made `kernel_sha256` independent of the dev *shell*. It was still
per-*platform*: the same checkout, the same `flake.nix`, and the same pinned seL4
source produced `e8cbab4f…` on `aarch64-darwin` and `f2d316e1…` on
`aarch64-linux`. The cause was not a leak but the toolchain: `flake.nix` names
`pkgsCross.aarch64-multiplatform.stdenv.cc`, which resolves to a **cross**
`gcc-wrapper` on Darwin and a **native** `gcc` on `aarch64-linux`, and Darwin's
wrapper forces `-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer` through
`nix-support/cc-cflags-before`. Every function prologue differed.

Fixed by having the build state its own frame-pointer policy rather than
inherit whichever the platform's wrapper imposes. `aarch64-darwin`,
`aarch64-linux`, and `x86_64-linux` now produce a byte-identical `kernel.elf` at
`97dcb029…`, `cmp`-verified, and B19's shell-independence still holds on each.

## Observable symptom

- Command: `python3 scripts/build/build-sel4.py`'s configure/build/install phase
  on each platform, then compare `build/sel4-prefix/bin/kernel.elf`.
- Expected: one `kernel_sha256` for a given seL4 source, config, and compiler
  version.
- Observed: `e8cbab4f…` on `aarch64-darwin`, `f2d316e1…` on `aarch64-linux`. The
  other four pinned artifacts — both `gen_config.json`, `kernel.dtb`,
  `platform_gen.yaml` — matched exactly, so configuration and device tree were
  already platform-independent.
- Evidence: `just sel4_qemu_image_check` can only pass on the platform that
  recorded the pin; on the other it reports drift that is real but uninteresting.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Both hosts report `aarch64-unknown-linux-gnu-gcc (GCC) 15.2.0`, the same `-dumpmachine`, the same `--with-arch=armv8-a --enable-default-pie`, and the same `.ident` | The compilers are the same version; the difference is in how they are invoked. |
| 2 | Darwin resolves `.../aarch64-unknown-linux-gnu-gcc-wrapper-15.2.0/bin/...`; `aarch64-linux` resolves `.../gcc-15.2.0/bin/gcc` — a native compiler with an empty `targetPrefix` | The two platforms do not use the same wrapper, which is B19's recorded `CROSS_COMPILER_PREFIX` gap seen from the other side. |
| 3 | Darwin's `nix-support/cc-cflags-before` is `-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer -march=armv8-a` | Three injected flags to account for, all *before* the command line. |
| 4 | One trivial TU compiled with identical explicit flags: Darwin emits the `stp x29, x30` / `mov x29, sp` / `ldp` prologue and epilogue plus `.cfi` directives; Linux emits the bare `add w0, w0, 1; ret` | The frame pointer is the whole delta, and it is in every function. |
| 5 | Darwin's **unwrapped** GCC on the same TU yields `73838445…`, byte-identical to Linux's native GCC | The compilers agree. Only the wrapper's injected flags differed — this is not a compiler-build difference. |
| 6 | The **wrapped** Darwin compiler plus `-fomit-frame-pointer -momit-leaf-frame-pointer` also yields exactly `73838445…` | A command-line flag defeats a `cc-cflags-before` entry, so the build can settle the question itself. |
| 7 | `-march=armv8-a` is set by seL4 itself (`deps/sel4/CMakeLists.txt:77-78`) and equals the compilers' own default (`gcc -Q --help=target`), and appears once on the real kernel compile edge | That injection is inert; it needs no counter-flag. |
| 8 | `nix-support/{libc-cflags,libc-crt1-cflags,cc-cflags}` carry `-idirafter`/`-B` paths for glibc and the gcc lib dir | The kernel builds `-nostdinc -ffreestanding -nostdlib`, so these reach nothing it compiles or links. |

## Root cause

`configure_and_install_sel4` stated the flags it cared about — the prefix maps
and, after B19, the random seed — and let everything else come from the
compiler. On a platform whose nixpkgs wrapper injects a code-generation flag
through `cc-cflags-before`, "everything else" includes codegen policy. Because
`cc-cflags-before` is prepended ahead of the command line, the injection is
invisible to the environment scrub B19 added: it is not an environment variable,
it is a file inside the wrapper derivation.

The violated invariant is the one `[observed_prefix]` asserts — that the pinned
hashes are a function of the pinned inputs. The frame-pointer policy was an
unpinned input, supplied by whichever wrapper the host happened to resolve.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/build-sel4.py` | `common_flags` gains `-fomit-frame-pointer -momit-leaf-frame-pointer`, so `CMAKE_C_FLAGS`/`CMAKE_ASM_FLAGS` state the policy instead of inheriting it. Fourth bullet added to the leading comment. | Codegen policy is repo-controlled, not platform-controlled. |
| `sel4/pins.toml` | `kernel_sha256` re-observed as `97dcb029…`; the comment now claims platform independence and names the compiler *version* — not the wrapper — as what it binds. | The recorded pin is reproducible on more than the recording platform. |
| `flake.nix` | Comment-only: the per-platform difference in what `crossCC` resolves to no longer reaches the kernel. | Prose matches behavior. |

These flags are a policy the build **chooses**, not a compiler default it
restores, and they move *both* platforms rather than only Darwin. GCC's aarch64
backend disables `-fomit-frame-pointer` at every `-O` level
(`aarch_option_optimization_table`, `OPT_LEVELS_ALL`), so an aarch64 kernel keeps
its frame pointers at `-O2` unless the flag is explicit. Measured on the pinned
GCC 15.2.0, a non-leaf function emits one `mov x29, sp` at `-O0`, `-O1`, `-O2`,
`-O3`, and `-Os` alike, and zero with `-fomit-frame-pointer`.

The choice is sound rather than merely convergent: seL4 states no frame-pointer
preference anywhere in `deps/sel4/`, and nothing walks one. The only `x29`
references in the AArch64 trap path (`c_traps.c:58`, `traps.S:115`) store `x29`
as *user* state at fixed `sp` offsets, unaffected by the kernel's own frame
policy; `Arch_userStackTrace` — live, since the installed `kernel/gen_config.h`
has `CONFIG_DEBUG_BUILD 1` and `CONFIG_PRINTING 1` — computes
`sp + i * wordsize` linearly and never dereferences a saved `x29`; and
`slime-root`'s "unwind" mentions are all prose about error paths.

**Debugger backtraces survive omission**, which is the stronger form of this
argument and worth stating precisely: seL4 builds
`-fno-asynchronous-unwind-tables`, but that suppresses only *asynchronous*
tables. `kernel.elf` still carries `.eh_frame` with 418 FDEs, and GDB and LLDB
unwind aarch64 from CFI rather than from the frame-pointer chain. A frame pointer
would only matter to a runtime in-kernel walker, and there is none. Omitting it
buys back a register and a prologue per function at no cost to debuggability.

`-momit-leaf-frame-pointer` is belt and braces: under `-fomit-frame-pointer` no
function gets a frame pointer, leaf or not, and the two flags together emit
assembly byte-identical to the first alone (verified). It is kept because it
names the second of the wrapper's two injections. Neither flag converges the
platforms on its own.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A platform's wrapper reintroduces a codegen flag | `just sel4_qemu_image_check` | `kernel.elf` SHA-256 mismatch against `[observed_prefix]` |
| The frame-pointer flags are dropped | `just sel4_qemu_image_check` | fault-injected: replacing the flag string with `""` reverts Darwin to `e8cbab4f…` and Linux to `f2d316e1…` |
| The fix regresses B19 | `just sel4_qemu_image_check` | a hostile-environment build no longer matches the pin |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| **Exit condition:** `kernel.elf` built on `aarch64-darwin` vs `aarch64-linux` vs `x86_64-linux` | `cmp` reports **byte-identical** across all three, each 973184 bytes, `97dcb029127bcc0fac1fd6cc83950ba878040a773a380e2d000b0ebc20d875ce`. Three different dev-shell seeds — `r279wlb3cq`, `65gzz0x3v8`, `6ckb6q72lb` | Direct |
| `just sel4_qemu_image_check` on `aarch64-darwin` | Pass — the `--prefix` pin check inside it verified `97dcb029…` | Direct |
| The other eight `sel4_*` Justfile gates (`sel4_pin_check`, `sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_channel_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_sample_check`, `sel4_stream_check`) | Pass | Direct |
| B19 property, `aarch64-darwin`: real shell vs hostile environment (fabricated `-frandom-seed`, fake `-isystem`, `NIX_HARDENING_ENABLE`, `CFLAGS`, `ASMFLAGS`, `NIX_SET_BUILD_ID=1`, `CMAKE_INCLUDE_PATH`) | Byte-identical `97dcb029…` | Direct |
| B19 property, `aarch64-linux`: real shell vs the same hostile environment | Byte-identical `97dcb029…` | Direct |
| Fault injection, `aarch64-darwin`: flag string replaced with `""` | `e8cbab4f…` — exactly the pre-B20 Darwin hash | Direct |
| Fault injection, `aarch64-linux`: same | `f2d316e1…` — exactly the pre-B20 Linux hash. Note this moved too: the flags change both platforms, not only Darwin. | Direct |
| Both flags present on every kernel compile edge | 15 of 15 edges in `build/sel4-qemu/build.ninja`, C and ASM | Direct |
| Other four pinned artifacts | Unchanged on both platforms, matching the values recorded before B19 | Direct |
| `just ruff`, `just typos`, `just fmt_check_all`, `just lint_all`, `just devlog_check` | Pass | Direct |

Both Linux hosts are OrbStack's Docker engine running `nixos/nix:latest` (Nix
2.35.1) over this checkout and the same `flake.nix`: one `linux/arm64`
(`uname -sm` = `Linux aarch64`), one `linux/amd64` (`Linux x86_64`, Rosetta-backed,
which needs `--option sandbox false --option filter-syscalls false` because Nix's
seccomp filter will not load under emulation). The three hosts differ in
dev-shell hash as well as platform, and `x86_64-linux` is the case that matters
most for the toolchain question: there `pkgsCross.aarch64-multiplatform.stdenv.cc`
is a genuine *cross* wrapper, as on Darwin, rather than the native `gcc`
`aarch64-linux` resolves — so the set covers both wrapper shapes.

**These are Linux kernels under a macOS hypervisor, not separate hardware** —
the right test for toolchain and shell independence, which is all this entry
claims, and no evidence about physical-board reproducibility.

## Decisions

- Decision: state the frame-pointer flags in the build, rather than change which
  toolchain `flake.nix` names.
- Rationale: B20 proposed naming one cross toolchain that resolves identically on
  every system. That is a larger change with a worse failure mode — it would
  make the Darwin and Linux shells fetch a toolchain neither platform's nixpkgs
  selects by default, and it moves the recorded hash for a reason unrelated to
  the defect. Stating the flag is one line and makes the *build* the authority on
  codegen policy, which is where that authority belongs. The pinned artifact then
  depends on the compiler version rather than on the wrapper.
- Correction from review: an earlier draft justified this as "restoring GCC's
  `-O2` default." **That was wrong**, in all five places it appeared, and the
  error is worth recording because the misleading evidence is easy to repeat:
  `gcc -O2 -Q --help=optimizers` prints `-fomit-frame-pointer [enabled]` on
  aarch64 while codegen still emits the frame pointer. `aarch64.cc` drives
  codegen off a tri-state in which only an *explicit* flag omits it, and
  `aarch_option_optimization_table` disables the option at `OPT_LEVELS_ALL`. The
  patch therefore moves the `aarch64-linux` hash as well — which this entry's own
  fault injection already showed (`f2d316e1…` → `97dcb029…`) and the draft's
  wording contradicted.
- Rejected alternative: recording a hash per platform. B19 argued a per-host pin
  cannot fail for the reason it exists; a per-platform pin has the same defect in
  a smaller form.
- Rejected alternative: adding `-fno-omit-frame-pointer` to match Darwin instead.
  It would equalize the platforms just as well and is a defensible choice — but it
  keeps a frame pointer no consumer in this tree reads, at a cost of one register
  and a prologue per function. The reason that is safe is *not* that seL4 gave up
  on unwinding: `.eh_frame` survives with 418 FDEs and debuggers unwind aarch64
  from CFI, so backtraces work either way. Omitting is the better default *for
  this kernel*; neither direction is the compiler's default on aarch64. Reviewed
  a second time on this question specifically, and kept.
- Note on scope: the other two things Darwin's wrapper injects need no
  counter-flag, and the entry records why rather than leaving it implied.
  `-march=armv8-a` is what seL4 passes itself and what both compilers default to;
  the glibc/gcc `-idirafter` and `-B` paths reach nothing in a `-nostdinc
  -ffreestanding -nostdlib` build.

## Open risks and follow-ups

- [ ] **The fix neutralizes today's wrapper difference; it does not make the two
      wrappers the same.** A future nixpkgs change that adds a different
      `cc-cflags-before` entry on one platform would reintroduce divergence, and
      the gate would report it as toolchain drift without saying which platform
      was odd. That is a strictly better failure than the silent one B19 left,
      but it is not a guarantee. Making `flake.nix` name one wrapper for both
      platforms remains the stronger fix, and is now optional rather than
      required.
- [ ] `[observed_prefix]` still binds build tooling `sel4/pins.toml` does not
      pin — `cmake`, `ninja`, and the host Python generators. Carried forward
      from B19 unchanged.
- [ ] Three platforms observed, all on one machine, two of them virtualized and
      one of those emulated. The `x86_64-linux` case that this entry originally
      recorded as **[INFERENCE]** is now **observed** and agrees, so both wrapper
      shapes — genuine cross and native — are covered. What is still unobserved
      is a second *physical* machine.
- [ ] B12, B16, and B17 remain open, unrelated to this.

## Artifacts and provenance

- Backlog entry: `roadmap/00-backlog.md`, resolved log, B20.
- Predecessor: `devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/` — whose
  `## Corrections` section opened this defect, and whose pin this supersedes.
- Related roadmap item: `roadmap/07-architecture-portability.md` — P5, whose
  `[observed_prefix]` gate this widens from one platform to three.
