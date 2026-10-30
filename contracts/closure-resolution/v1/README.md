# Closure resolution report format 1

A resolution report says what one closure resolved to in one checkout. `schema.zt` is the normative
layout: the closure's `name`, its `derivationId`, and whether the resolution was `dirty`.

`python3 scripts/generate/generate-system-image-closures.py --resolve NAME --output PATH --report REPORT`
writes one report to REPORT. The resolved closure at PATH is a `contracts/system-image-closure/v2`
`SystemImageClosure`, and `derivationId` is its identity. A resolution is dirty if a tracked file under
one of the closure's inputs is modified, or an untracked or ignored file sits under one. A dirty
resolution may be built and tested, but no evidence or attestation may name it.

A judge in another process reads the report back through `decodeResolutionReport`, never by parsing it
by hand. `just closure_derivation_check` is that judge. Work item `01a18cca-71e6-74e8-9161-ab2736a67bc1`
implements the resolver. The design is in
[`docs/plans/system-image-closure-derivations.md`](../../../docs/plans/system-image-closure-derivations.md).
