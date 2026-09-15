# Development history and evidence

Current architecture, operation and limitations remain in this product repository.
`.tasks/items/` is the only live work-state store. Historical investigations and
large raw captures are preserved in a private repository, not a build, test,
submodule or documentation-check dependency.

## Immutable lookup

Repository identity:
`ssh://git@git.justaslime.dev:2222/iceice666/slime_os-history.git`

Product archive anchor: `45ed1745907b2d0a13fdf70c8b34eb635bed5f23`.
Tree: `e2e84b2fca59ed5221b0084aeb986dced4aeadae`.

Use repository identity + full commit + original path. For example, the
[original devlog navigation](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/devlog/README.md)
retains its original surrounding source tree and links. Do not use mutable main
for an evidence locator. Access requires permission to the private archive;
product checks never fetch it or treat host availability as evidence.

[Archive restoration and inventory](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/e3aa4d42d87453b78286d9f6035f1b296f598811/README.md)
explain the daily branch, immutable snapshot tags, independent backup and limits.
The daily branch contains no product workflows or live task store. The owner
confirmed private visibility and disabled automation and authorized preservation
of personal/device metadata. No live secret was identified by targeted review;
this is not an exhaustive secret-absence guarantee. Do not broaden access.

## What moved and what remains

All 491 historical devlog files and the frozen backlog index were verified
against the archive before removal. Other roadmap files remain because their
[classification](../roadmap/README.md) explicitly retains unextracted requirements.
Do not move those requirements into a private historical archive by implication.

The exact Duo transcript remains a product checker fixture at
[`evidence/duo/maee-smoke.log`](../evidence/duo/maee-smoke.log), SHA-256
`fc9802dbf74941d000f39429928ccddf06b6a2cf8188438415216741bfd3e801`.
The Framework observation remains in [`evidence/framework-cpu-boot/`](../evidence/framework-cpu-boot/)
with unchanged bytes, digest, tested-image binding and refusal semantics. This
migration makes no new QEMU or physical-machine claim. The Framework observation
remains about a superseded image until a new operator boot is observed.

The archive includes the 14 reachable product commits and a separate read-only
legacy GitHub snapshot with two commits. Older referenced commits remain
unresolved in those histories; no mapping was fabricated. Gitlinks are preserved,
but submodule object databases, hosted artifacts and external attachments are not
included. One existing archival link, the IO TCP plan's `folder/index.md`, remains
broken and is recorded in the archive inventory. A snapshot does not establish
that every historical environment is reconstructible.

## Current checks and rollback

`just docs_check` checks current local links/fragments, canonical UUID references,
and current commands. Historical prose is not validated against today's recipes;
its original checker stays at the archive revision. `just tasks_check` runs real
MyQue integrity checks and backlog-first policy without a frozen index. Neither
requires history checkout or network access.

The archive's `verify-archive.py` compares every candidate original with its Git
blob and rejects missing/mismatched bytes; it never deletes files. Local complete
bundles and an independent remote restoration were verified before cutover.
Same-machine backups are not an off-site guarantee, and remote protection was
not inspected or modified. Roll back product removal and consumers together by
reverting the cutover commit, without force-push or rewriting either history.

New exceptional investigations belong in the history repository. Record tested
revision and image digest, target/config, command, observations and limits,
artifact locator/digest, and retention. Ordinary changes require no investigation
or transcript; update owning product docs and canonical work-item evidence.
