# Gate diagnostic receipts

`v1/schema.zt` owns the local receipt an executed recipe's diagnostic capture
produces. It is not acceptance evidence and cannot authorize closure.

The adapter keeps bounded opaque output beside a Zutai immediate receipt.
Output is the prefix of the recipe's stdout followed by its stderr, limited to
`limitBytes`; `truncated` explicitly says whether that limit dropped bytes.
`recipeExitCode` is the observed exit of `just <target>`. The receipt binds the
complete execution identity, the recipe, run key and output digest. Neither the
receipt nor its output may be overwritten by another execution.

The operator entrypoint is `scripts/lib/devloop_cli.py`; the exam imports
`scripts/lib/devloop.py` only for pinned toolchain and consumer helpers. It copies
the entrypoint as the program under test, without importing it into the checker's
immutable verification closure. Both files remain under the evidence code closure;
there is no approval exemption.

The real operator-facing `just devloop gate` command must expose this line on its
stdout or stderr, including on reuse of a failed run:

```text
SLIME_DEVLOOP_DIAGNOSTIC receipt=<repository-relative-path> run=<run-key>
```

Both paths must remain inside the checkout, contain no symlink, and satisfy the
schema bounds. The repository operator entrypoint relays only discovery and
capture-error messages around the pinned CLI's stderr capture; typed observation
stdout remains untouched. Recipe subprocesses do not inherit that relay channel.
A capture/retention error is a gate refusal with an attributable
`diagnostic ... error/refused/unavailable` message, not recipe success. Local raw
logs are not uploaded or published by the gate.

The owner is backlog `01a11bda-a60d-7234-a3b9-34f11911cf37`.
`just devloop_diagnostics_check` grades the actual pinned CLI in an isolated
MyQue fixture store. `just devloop_diagnostics_controls` tests the judge's
negative controls and never qualifies the repair. The recipe runs the canonical
store check first. The qualification covers the adapter's receipts and the operator's discovery path.

The retained output limit is per capture, not an aggregate storage quota. Full
stdout/stderr are temporarily spooled to disk to preserve the existing
`just-observations` full-text digest (including universal-newline normalization)
without unbounded transcript memory. Valid UTF-8 keeps the prior digest; invalid
UTF-8 is round-tripped with surrogate escapes so opaque diagnostic bytes are not
lost before retention. Temporary disk use is not bounded by the
retained limit. The legacy `work-item-store` count parser still keeps complete
checker text in memory while retaining bounded logs for both recipes.
Existing run-cache reuse is sequential; concurrent same-identity
invocations are not guaranteed single execution. Each retained capture uses its
own directory and never replaces a predecessor's files.
