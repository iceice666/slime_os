# `sel4-sample.zti` — statically bound sample composition

The sample plane launches `sample-receiver` and `sample-lender` with empty spawn
grant lists. Runtime automatically installs the fixture's minted bindings:
receiver peer endpoint at slot 0; lender peer endpoint at slot 0, shared-buffer
factory at slot 1, and narrowed receiver supervision authority at slot 2.
The peer path is therefore a direct native Endpoint rather than a pair created
by init and handed to the children.

The lender creates, maps, fills, seals, and loans a payload larger than the
64-byte control-message bound. It sends the typed sample descriptor plus the
single loan capability; the receiver validates the record, maps and verifies
the payload read-only, returns the loan exactly once, and sends `settled`.
Nonblocking receive loops yield on `ERR_WOULDBLOCK`; no endpoint wait source is
used.

`startup_arg` distinguishes the spawned receiver body (`1`) from the
generation's root-autostart copy (`0`). The latter reports its probe-skip marker
instead of inferring its role from a missing runtime-granted endpoint. Init
retains the existing live-instance refusal and post-exit budget-recovery causal
markers.
