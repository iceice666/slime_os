## Why

<!-- 1–3 sentences. -->

## What changed

-

## Invariants / risk

<!-- Delete if trivial. -->

-

## Verification

<!-- Name exact commands/scenarios, observed result, and evidence class
     (direct / inherited / inference). For hardware or image claims, name the
     tested target, revision, and binary identity. State known limits. -->

| Check / scenario | Observed result | Evidence class / scope |
|---|---|---|
| `just ...` | pass | direct |

## Related

<!-- Work item UUID from `.tasks/items/` (an optional human key may follow in
     parentheses); the UUID is the canonical machine identity. Existing
     evidence or decision links may follow. Ordinary changes need no devlog. -->
-

<!-- This Related section is not machine parsed. After opening the PR, use:
     nix run github:mozufu/myque-gh/c1da385362d1eca3dc1bdf52b4c499e68d3007a4#myque-gh -- pr link <PR> <ITEM> --store . --repo iceice666/slime_os --ref HEAD --apply
     <ITEM> accepts a human key; UUID remains the canonical machine identity.
     Requires authenticated gh and permission to edit the PR body. The command
     manages a trailer at the end of the body; do not hand-edit it. Projection
     refreshes automatically on PR edits/lifecycle events and CI completion.
     A scheduled fallback catches review changes and missed/token-generated
     events. workflow_dispatch is an operational escape hatch, not a contributor
     step.
     Do not use GitHub auto-close keywords for projected Issues. Merging a PR
     does not complete its item; use myque close only after recording the
     observed exit evidence, its identity, scope, and limits in the item. -->
