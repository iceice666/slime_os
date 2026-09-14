# Repo-owned seL4 Rust target specifications

Every other admitted seL4 profile names one of `deps/rust-sel4`'s own target
specifications, pinned by path and SHA-256 in `sel4/pins.toml`. The two files
here are the exception, and they exist for one reason.

rust-sel4's `x86_64-sel4-*.json` specifications pair

```json
"features": "-mmx,-sse,-sse2,-sse3,-ssse3,-sse4.1,-sse4.2,-avx,-avx2,+soft-float",
"rustc-abi": "softfloat"
```

That combination has no LLVM lowering for the 128-bit integer arithmetic
`curve25519-dalek` performs, so `slime-root` — which links `ed25519-dalek`
through `boot-contracts`' `release-crypto` feature to verify generation release
signatures — fails in codegen rather than in type checking:

```
rustc-LLVM ERROR: Do not know how to split the result of this operator!
error: could not compile `curve25519-dalek` (lib)
```

Re-enabling SSE while keeping `rustc-abi = "softfloat"` fails differently
(`Unknown mismatch in getCopyFromParts!`), and dropping only the `+soft-float`
LLVM feature while keeping the softfloat ABI fails in `core` itself
(`SSE register return with SSE disabled`). The working configuration is a
hardware-float ABI with SSE2 — the x86-64 baseline — and no `+soft-float`.

That is also the correct configuration for this platform rather than a
workaround. seL4 pc99 sets `CONFIG_HAVE_FPU` and saves and restores x87/SSE
state per thread through `XSAVE` (`KERNEL_X86_FPU = "XSAVE"` in the installed
`kernel/gen_config.json`), so a userspace task may use floating-point and
vector registers. The AArch64 and RV64 profiles keep rust-sel4's softfloat
specifications because their pinned kernels export no FP context to save.

Each file is otherwise byte-derived from its rust-sel4 counterpart: only
`features` is rewritten and `rustc-abi` removed. `scripts/check/check-sel4-pins.py`
validates both against the pinned hashes and asserts exactly this delta, so an
upstream specification change cannot silently diverge from these copies.
