# `sel4-loan.zti` — direct endpoint loan plane

Init acts as lender and uses the generation-declared endpoint at slot 4 to the
receiver's slot 0. The receiver is spawned with an empty grant list because the
runtime installs its declared endpoint automatically. Init's shared-buffer
factory and the unrelated console edge remain statically bound by the fixture.

The plane preserves the quota and authority checks: factory-kind refusal; page,
buffer, mapping, and loan ceiling refusals; sealing; receiver identification by
capability; undelegated receiver refusal; typed descriptor validation; exact
one-cap loan transfer; read-only mapping; single return; and reclamation after
the receiver's `settled` response. Receive polling yields on
`ERR_WOULDBLOCK`; no endpoint wait source is used.

Direct endpoint send is rendezvous, so the retired logical-queue experiment
that stranded a loan in root transit has been removed. The causal markers still
cover payload creation and verification, loan transfer and settlement, quota
reclamation, and the unrelated console holder proving its independent budget
remains intact.
