# System-image closures as derivations

This plan is unimplemented. It moves the system-image closure corpus from committed input digests to
committed sources, so a change to an input tree stops rewriting derived records. The decision and
its alternatives are in
[`decisions/system-image-closures-as-derivations.md`](../decisions/system-image-closures-as-derivations.md).
Step 1 is work item `01a18732-ad06-744a-85e5-d50c903828f9`. Steps 2 and 3 have no item yet.

## Problem

A closure under `contracts/system-image-closure/v2/closures/` records a SHA-256 identity for every
input tree, including the repository's own trees such as `components/lib`. Each test-run record and
each negative build case copies a closure identity. A change to any input tree therefore rewrites
174 files: 103 closures, 6 negative build cases and 65 test-run records. Two open pull requests that
touch an input tree conflict on those files, whatever their content. At `00af4fde`, 86 of the last
224 commits on `main` touch the closures.

The committed digests of in-tree inputs add no pin. The commit already fixes those bytes. Only inputs
outside the repository need a recorded pin: `sel4/pins.toml`, the submodule gitlinks, `flake.lock`
and the toolchain.

## Target model

| Layer | Content | Location | Changes when |
| --- | --- | --- | --- |
| Closure source | composition, implementations, roles, build parameters, input locators | `contracts/system-image-closure/v3/closures/`, committed | a composition changes |
| Resolved closure | the source, one SHA-256 identity per input, and the derivation id | computed from the checkout, never committed | an input changes |
| Attestation | commit, derivation id, host class, output digests | the git notes ref `refs/notes/image-attestations` on the origin | image bytes change |

The rules:

- **In-tree identity.** An input's identity keeps today's `tree_digest` encoding over SHA-256. The file
  set is what `git ls-files --cached --others --exclude-standard` lists, so an ignored file is not part
  of the identity.
- **Dirty builds.** A build is dirty if a tracked file under an input is modified, or an untracked or
  ignored file sits under an input. A dirty build may run any gate locally. Its build result records
  `dirty=true`, and no attestation or devloop evidence may name it.
- **Derivation id.** On a clean tree the derivation id equals today's `closureIdentity`. Build caches,
  the build result and the SDK release record keep their meaning.
- **Test runs and negative cases.** A test-run record names its closure. A negative build case names
  its base closure.
- **SDK.** The published SDK carries a fully resolved closure, rendered from the release commit.
- **Attestations.** After a merge, a workflow builds every closure at the merged commit and writes one
  note per commit. Two runs that race on the notes ref merge with `git notes merge -s cat_sort_uniq`.
  For one host class, an equal derivation id must give equal output digests. PR CI does not read
  attestations. It keeps the check that two builds of one closure agree.
- **Host class.** The host class is the `flake.lock` narHash and the runner architecture.

## Steps

| Step | Change | Derived files no longer rewritten per input change | Gate |
| --- | --- | --- | --- |
| 1 | Test-run records and negative build cases name their closure (`contracts/system-test-run/v2`) | 65 + 6 | `just system_test_run_reference_check` |
| 2 | Closure v3 holds locators only. The resolver is the closure generator, called in memory over the git-visible file set, with the dirty rule. | 103 | to be admitted |
| 3 | Attestation contract, the notes writer workflow and its check | none; adds evidence across commits | to be admitted |

Step 1 precedes step 2, because the runs must stop copying identities before the closures stop
recording them. Step 2 precedes step 3, because an attestation names a derivation id. A scheduled
rebuilder of attested commits is deferred.

## Measured feasibility of step 2

These measurements were taken at `ffb4dc67` on one darwin host, with a read-only spike that is not part
of the repository:

- The git-visible digest equals today's walk digest, and the committed identity, for all 124 tree
  inputs the closures name.
- Over `components/lib`, the two digests agree on a modified file, an untracked file, a deleted file and
  an added executable bit. They differ only when an ignored file is present: the walk includes it and
  the git-visible digest leaves it out. The dirty rule covers that case.
- Rendering one closure in memory with the existing generator takes 0.02–0.3 s with a warm Zutai
  cache, and 6.4 s for a closure over a computed system. With the cache disabled it takes 9.9 s and
  16.5 s. Every rendered closure equaled the committed file byte for byte.

## Invariants

- The build key is a pure function of declared inputs.
- A clean build is reproducible within one host class.
- A dirty build never becomes evidence.
- A scenario's build parameters and profiles stay part of the closure source and the derivation id.
- A negative build case is never a system-image closure.
- An SDK consumer builds without this repository's history.
- The attestation crosses a persistence boundary, so it has a Zutai contract.
- A closure proves bytes. A test run proves behavior. An attestation proves bytes across commits
  within one host class, and nothing about a boot.

## Known limits

- Git notes are not shown in GitHub's web interface. A clone fetches them only when asked to.
- A clone without submodules cannot resolve a submodule input.
- Cross-host comparison waits for a hermetic builder.
