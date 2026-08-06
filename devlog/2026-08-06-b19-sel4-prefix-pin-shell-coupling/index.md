# B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain

| Field | Value |
|---|---|
| Date | 2026-08-06 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/build/build-sel4.py`, `sel4/pins.toml`, `flake.nix` |
| Roadmap | B19 |
| Gates | `just sel4_qemu_image_check` |
| Trigger | B19 opened 2026-08-06 (`8fc61eb`) after `kernel_sha256` failed to reproduce on `aarch64-darwin` |
| Baseline | `[observed_prefix].kernel_sha256 = 2d88b9a4…`, which did not reproduce here. *Inherited from `8fc61eb`:* recorded on an `x86_64-linux` host; that attribution was not re-observed in this work. |

## Summary

`sel4/pins.toml`'s `[observed_prefix]` is the gate that would notice a change of
seL4 compiler. It did not do that: it pinned the **dev shell's own derivation
hash**, because `configure_and_install_sel4` inherited `os.environ` and nixpkgs
puts `-frandom-seed=<first 10 chars of the devShell derivation hash>` into
`NIX_CFLAGS_COMPILE`. GCC uses that seed for symbol and section naming, so
adding a tool to `flake.nix` — or reordering the list — changed `kernel.elf`
byte-for-byte and was reported as toolchain drift. The same variable carried
`-isystem` store paths for every package in the shell, and
`NIX_HARDENING_ENABLE` imposed the shell's hardening policy on a freestanding
kernel that asks for none of it — of which one flag,
`-fzero-call-used-regs=used-gpr`, actually reached codegen; see *Decisions* for
what the other two did and did not do.

Fixed by building the kernel with the shell's build inputs **removed** rather
than by re-pinning per host, and by setting `-frandom-seed` to a fixed
repo-controlled value. The hash no longer depends on the shell that ran the
build. It does still bind the cross compiler and the build tooling
`sel4/pins.toml` does not pin — `cmake`, `ninja`, and the host Python generators
— which is recorded as a residual rather than claimed as closed. Re-observed on
`aarch64-darwin`: `e8cbab4f…`. Adding `hexdump` to `flake.nix`'s `packages`
moves the shell's seed from `r279wlb3cq` to `rhl1f441df` and leaves
`kernel_sha256` unchanged.

## Observable symptom

- Command: `just sel4_qemu_image_check` on `aarch64-darwin`.
- Expected: the installed prefix matches `[observed_prefix]`.
- Observed: four of five pinned artifacts matched exactly — both `gen_config.json`,
  `kernel.dtb`, and `platform_gen.yaml` — and only `kernel.elf` differed
  (`dc852cf9…` against the pinned `2d88b9a4…`), so configuration and device tree
  were already reproducible.
- Exit evidence: `check-sel4-pins.py --prefix` exits 1 with
  `build/sel4-prefix/bin/kernel.elf SHA-256 is …, expected …; rebuild with
  'just sel4_qemu_image_check' or inspect toolchain drift`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `printenv NIX_CFLAGS_COMPILE` in the dev shell begins ` -frandom-seed=r279wlb3cq`, followed by `-isystem`/`-fmacro-prefix-map` pairs for `lldb`, `qemu`, `dtc`, `python3`, … | The ambient shell contributes compiler flags the build never asked for. |
| 2 | `nix print-dev-env --json` reports the same seed `r279wlb3cq`, and `nix eval .#devShells.aarch64-darwin.default.drvPath` resolves; the seed is the devShell derivation hash's first 10 characters | The seed is a function of the shell's package list, not of the compiler. |
| 3 | `aarch64-unknown-linux-gnu-gcc -### -c t.c` shows `-frandom-seed=r279wlb3cq`, `-fstack-protector-strong`, `-fzero-call-used-regs=used-gpr`, `_FORTIFY_SOURCE=3`, and 60 `-isystem` entries | The flags reach the actual cross compiler the kernel builds with, so they are in `kernel.elf`. |
| 4 | The wrapper's `nix-support/add-flags.sh` mangles `NIX_CFLAGS_COMPILE` into `NIX_CFLAGS_COMPILE_aarch64_unknown_linux_gnu`; `add-hardening.sh` reads `NIX_HARDENING_ENABLE_aarch64_unknown_linux_gnu` | The scrub must cover the mangled and role-suffixed spellings, not just the base names — a prefix match does. |
| 5 | `deps/sel4/CMakeLists.txt:181-195` states `-ffreestanding`, `-fno-stack-protector`, `-fno-common`, `-nostdinc` | The kernel states its own flags and asks for none of the shell's hardening; dropping the set restores the kernel's stated intent rather than changing it. |
| 6 | `build/sel4-qemu/CMakeCache.txt` shows `CMAKE_C_FLAGS`/`CMAKE_ASM_FLAGS` carrying only the two `-ffile-prefix-map` entries | CMake's flags were already repo-controlled; the leak was entirely environmental. |

## Root cause

`configure_and_install_sel4` called `run(...)` without an `environment`
argument, and `run` passes `env=None` to `subprocess.run`, which inherits the
parent environment verbatim. nixpkgs' cc-wrapper then reads
`NIX_CFLAGS_COMPILE` and `NIX_HARDENING_ENABLE` out of that environment and
appends their contents to every compiler invocation. `-frandom-seed` is the
decisive one: GCC uses it to seed the symbol and section names that must differ
per translation unit, so its value is observable in the output ELF.

The violated invariant is the one `[observed_prefix]` exists to assert — that
the pinned hashes are a function of the pinned *inputs*. They were a function of
the dev shell as well, which is why the gate could not distinguish a real
compiler change from an unrelated `flake.nix` edit, and why the pin reproduced
only on the host that recorded it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/build-sel4.py` | `sel4_build_environment()` builds the kernel's environment from `os.environ` minus every `NIX_CFLAGS*`/`NIX_LDFLAGS*`/`NIX_CXXSTDLIB*`/`NIX_FFLAGS*`/`NIX_GNATFLAGS*`/`NIX_HARDENING_ENABLE*` variable, the four `CFLAGS`-family names CMake seeds `CMAKE_<LANG>_FLAGS_INIT` from (`ASMFLAGS`, not `ASFLAGS`), the bintools wrapper's `NIX_SET_BUILD_ID`/`NIX_BUILD_ID_STYLE` switches, and `CMAKE_INCLUDE_PATH`/`CMAKE_LIBRARY_PATH`/`CMAKE_PREFIX_PATH`. Passed to all three `cmake` invocations. | The kernel's build inputs come from the repository, not from the ambient shell. |
| `scripts/build/build-sel4.py` | `-frandom-seed=slime-sel4-qemu-arm-virt` appended to `CMAKE_C_FLAGS`/`CMAKE_ASM_FLAGS` beside the existing prefix maps, and an explicit empty `-DCMAKE_EXE_LINKER_FLAGS=`. | GCC's symbol-naming seed is repo-controlled and identical on every host; no stale cache can retain a shell's `LDFLAGS`. |
| `sel4/pins.toml` | `kernel_sha256` re-observed as `e8cbab4f…`; the comment now states what the pin binds, what it still does *not* pin, and which host observed it. | The recorded pin is reproducible off the recording host. |
| `flake.nix` | Comment-only: the claim that reordering `packages` would change every Linux `kernel.elf` is no longer true, and said so. | Prose matches behavior. |

Prefix-matching rather than an exact name set is deliberate: the cc-wrapper
mangles each variable by target and role (`NIX_CFLAGS_COMPILE_aarch64_unknown_linux_gnu`,
`_FOR_BUILD`, `_FOR_TARGET`), so an exact list would miss the spellings the
wrapper actually reads. The three exact-name groups cover routes no prefix
catches, and only one of them is a live leak — `CMAKE_INCLUDE_PATH` is prepended
to `find_file` search order, which no `-D` protects, and a decoy directory
holding a `helpers.cmake` does win over seL4's in-tree `tools/helpers.cmake`
(verified in a scratch project). The other two groups are defense in depth and
labelled as such in the code.

Everything the build needs is kept: `PATH`, the wrapper's own target markers
(`NIX_CC_WRAPPER_TARGET_HOST_*`), `NIX_CC`, `NIX_BINTOOLS`, `NIX_STORE`, and the
Darwin SDK variables. The cross compiler finds its libc, crt, and include paths
through its `nix-support` files rather than the environment, and the build proves
it by completing.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A shell edit is reported as toolchain drift again | `just sel4_qemu_image_check` | `kernel.elf` SHA-256 mismatch against `[observed_prefix]` |
| A real compiler change goes unnoticed | `just sel4_qemu_image_check` | fault-injected: one nibble changed in `kernel_sha256` makes `check-sel4-pins.py --prefix` exit 1 |
| The scrub drops something the build needs | `just sel4_qemu_image_check` | configure/build/install failure, or a missing installed artifact |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_qemu_image_check` | Pass — wrote `build/slime-sel4.elf` and `build/slime-sel4.identity.json`; the `--prefix` pin check inside it verified `e8cbab4f…` | Direct |
| Clean rebuild in this host's real shell (seed `r279wlb3cq`) | `kernel_sha256 = e8cbab4f74bbdb761d7bffac35a4a4ae0edcc68733af7758d696544fe5991503` | Direct |
| Clean rebuild with `NIX_CFLAGS_COMPILE` carrying seed `zzzzzzzzzz` plus fabricated `-isystem`/`-fmacro-prefix-map` store paths, a narrowed `NIX_HARDENING_ENABLE`, and an ambient `CFLAGS=-DAMBIENT_SHOULD_NOT_REACH_KERNEL` | Byte-identical `e8cbab4f…` | Direct |
| **Exit condition, second half:** `hexdump` added to `flake.nix`'s `packages`, real `nix develop`, observed shell seed `rhl1f441df` | Byte-identical `e8cbab4f…`; `flake.nix` then restored | Direct |
| Fault injection: `kernel_sha256` mutated by one nibble | `check-sel4-pins.py --prefix` exits 1 and names both hashes; unmutated it exits 0 | Direct |
| `strings build/sel4-prefix/bin/kernel.elf \| grep -c '/Users/iceice666\|/nix/store'` | `0` — no host or store path survives in the ELF | Direct |
| Other four pinned artifacts (`kernel_config`, `libsel4_config`, `dtb`, `platform_info`) | Unchanged across every build, matching the values recorded before this change | Direct |
| `just sel4_root_boot_check` on the rebuilt image | Pass — `ordered generation, timer, task, IPC, fault, and ready markers observed on the pinned qemu-arm-virt profile`. This patch changes kernel codegen, so a booted gate is what shows the rebuilt kernel still runs. | Direct |
| Hostile environment after review: `NIX_SET_BUILD_ID=1 ASMFLAGS=-DLEAK_VIA_ASMFLAGS LDFLAGS=-Wl,--build-id=sha1 CMAKE_INCLUDE_PATH=/tmp/decoy-include`, clean rebuild | Byte-identical `e8cbab4f…`, no build-id note in the ELF | Direct |
| Fault injection with the scrub replaced by `dict(os.environ)`, same hostile variables | `70ee1359…` — a different kernel, so the scrub is load-bearing. `NIX_SET_BUILD_ID` and `ASMFLAGS` alone did **not** move it even then, so both are defense in depth rather than live leaks, and the code says so. | Direct |
| Round-2 review correction on *why* `NIX_SET_BUILD_ID` is inert | Not the kernel's `-Wl,--build-id=none`: the wrapper appends `--build-id=sha1` after it and the linker honours the last occurrence. `build/sel4-qemu/linker.lds_pp:589-592` discards `.note.gnu.build-id` outright, which is what actually removes it. Inherited from the reviewer's repro; the discard clause was read directly. | Inherited + direct |
| `find_file` redirection, scratch `project(t NONE)` with a decoy `helpers.cmake` | `CMAKE_INCLUDE_PATH=/tmp/decoy2` resolves to the decoy; unset resolves to the in-tree `tools/helpers.cmake`. This is a real route, not defense in depth — my first probe used a decoy directory containing no `helpers.cmake` and was a false negative. | Direct |

The first half of B19's exit condition — a pass on a host whose dev shell
derivation hash differs from the recording host's — is now satisfied in the
strong form rather than the stated one. The recorded hash was observed on
`aarch64-darwin`, and the property demonstrated is that *changing this host's
own shell hash does not move it*, which is what the stated condition was
reaching for. A second physical host was not used. **[INFERENCE]** a
`x86_64-linux` or `aarch64-linux` shell would reproduce `e8cbab4f…` given the
same cross-compiler version; the seed and `-isystem` differences that defeated
the old pin are exactly what is now dropped, but no Linux host was observed.

## Decisions

- Decision: drop the shell's flag variables and set a fixed `-frandom-seed`,
  rather than re-pinning `[observed_prefix]` per host or per platform.
- Rationale: a per-host pin is not a gate — it cannot fail for the reason it
  exists. Making the build independent of the shell is what turns the hash back
  into a function of the pinned inputs, and it fixes the hardening leak in the
  same move.
- Rejected alternative: keeping `NIX_CFLAGS_COMPILE` and merely overriding
  `-frandom-seed` later on the command line. GCC honours the last seed, so this
  would work for the seed, but leaves the per-package `-isystem` store paths and
  the hardening set in place — a narrower fix for a leak that has three parts.
- Rejected alternative: a fixed seed per translation unit. The seed only
  suffixes file-scope static and section names, and `kernel.elf` links five
  objects — `kernel_all.c` plus `head.S`, `traps.S`, `idle.S`, and
  `machine_asm.S` — so a shared value has nothing to collide with. Thirteen
  compile edges share the flags in all; the other eight are bitfield/pruning
  scaffolding and libsel4, never co-linked into the pinned artifact.
- Correction from review: **most of the hardening set was already inert.**
  `-fno-stack-protector` is appended after the wrapper's
  `-fstack-protector-strong` and wins, and `_FORTIFY_SOURCE` is a libc macro
  with nothing to attach to under `-nostdinc -ffreestanding`. The one flag that
  reached codegen is `-fzero-call-used-regs=used-gpr`, adding a
  `mov x16, 0` / `mov x17, 0` pair before every `ret`. Dropping the set is still
  right — the kernel asks for none of it — but the semantic delta is one flag,
  not three, and the code comment now says that.
- Note on scope: `dump_device_tree` is deliberately **not** given the scrubbed
  environment. It runs QEMU, not a compiler, and `dtb_sha256` already reproduced
  exactly across hosts before this change.

## Open risks and follow-ups

- [ ] No non-Darwin host has been observed since the fix. B19's exit condition
      as written names "a host whose dev shell derivation hash differs"; that is
      satisfied, but a genuinely different platform would be stronger evidence.
- [ ] `aarch64-linux` still cannot run the documented build: there
      `pkgsCross.aarch64-multiplatform.stdenv.cc` resolves to a native
      `gcc-wrapper` with an empty `targetPrefix`, so `CROSS_COMPILER_PREFIX` is
      empty. Pre-existing, recorded in B19's own analysis, and untouched here.
- [ ] B12 (`--remap-path-prefix` naming a path that does not exist) remains
      open and is a different leak on the frozen x86 component path.
- [ ] `[observed_prefix]` still binds build tooling `sel4/pins.toml` does not
      pin: `cmake`, `ninja`, and the host Python generators (`bitfield_gen.py`,
      `invocation_header_gen.py`, `hardware_gen.py`, …) whose output is compiled
      into `kernel.elf`. A `cmake` or `jinja2` bump in `flake.nix` can therefore
      still move `kernel_sha256` while the compiler is unchanged, reported as
      "toolchain drift". Narrower than the defect B19 closed — the shell's
      *package set* no longer matters, only the versions of tools the build
      actually runs — but it is the same class, and the pins comment now says so.

## Artifacts and provenance

- Backlog entry: `roadmap/00-backlog.md`, resolved log, B19.
- Related roadmap item: `roadmap/07-architecture-portability.md` — P5, whose
  `[observed_prefix]` gate this repairs.
- Predecessor: `devlog/2026-08-05-p5-5-2-stream-plane/` — the last slice to
  build through this path with the leak present.
