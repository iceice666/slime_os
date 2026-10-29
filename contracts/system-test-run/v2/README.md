# System test-run format 2

A test-run record declares how one plane gate executes one image: the execution profile, disks,
networks, devices, fault controls, timeout, marker contract and forbidden outcomes. `schema.zt` is
the normative layout. `gen_python.zt` renders `scripts/lib/system_test_run_contract.py`, and
`check.zt` decodes one record. `runs/` holds one record per plane-gate target.

Version 2 changes one field. A record names the image closure it exercises as `imageClosure`, which
is the closure's file name without `.zti`. Version 1 stored `imageClosureIdentity`, a copy of the
closure's identity. That identity changes whenever one of the closure's input trees changes, so
every record that booted the closure had to be rewritten, and two open pull requests that touched
any input tree conflicted on every record. A version 2 record changes only when its own execution
inputs change.

`imageClosure` is empty only for a declaration whose checker is explicitly closure-exempt.
`scripts/check/check-system-test-run.py` requires a named closure to exist and to be the closure the
run's plane gate builds, and it requires an exempt run's image to be one the aggregate gate declares
closure-exempt.

## Regenerating

`python3 scripts/generate/generate-system-test-runs.py --check` refuses a record that differs from
its plane gate. After an intended change to a gate's execution inputs, re-freeze the records with
`just system_test_run_bless`. A change to a closure input leaves the records unchanged.

`just system_test_run_reference_check` judges that property in a scratch checkout. The design is in
[`docs/plans/system-image-closure-derivations.md`](../../../docs/plans/system-image-closure-derivations.md).
