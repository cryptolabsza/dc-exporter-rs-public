# Public snapshot and publishing

## Status and repository boundary

The source destination is `cryptolabsza/dc-exporter-rs-public`, initially private.
The existing `cryptolabsza/dc-exporter-rs` repository remains private with its
history, tags, CI records and raw authority/evidence. Binaries and DEBs continue
in `cryptolabsza/dc-exporter-releases`; existing stable assets keep their original
provenance. A fresh source tree does not retroactively validate those assets.

Public visibility, live runner/secret isolation, rehearsal-log disclosure and
RTX 5090 hardware acceptance are separate owner gates. This document describes
the reviewed export mechanism; it is not evidence that those gates have passed.

## Exact commit-tree export

`scripts/public_export.py` uses Python's standard library and read-only Git blob
queries. It accepts a full immutable commit SHA, an approved baseline SHA and an
external `public-snapshot/1` approval containing the literal selected paths,
SHA-256 hashes, Git modes and nonempty reasons for each permitted change.
Mutable working-tree and untracked content are never export inputs. Git replace
objects are disabled during reads. Output is a new directory outside any Git
checkout; no `.git`, old ancestry, refs, tags or fork/import metadata is copied.

The literal `PUBLIC_PATHS` list in the script is the selection authority. It
includes all existing production Rust files (including legacy register code),
Rust tests, package/service/build assets, reviewed CI scripts/tests, public guides,
license/provenance and the public C layout fixture. All selected tracked paths
must be present in the approval. New source/test paths, omitted baseline assets,
symlinks, submodules, unsafe paths, mismatched hashes/modes and unexpected output
files/directories are rejected.

Every source/test/package byte and mode must equal the baseline, except the
explicitly permitted documentation, CI, license and new export/test paths.
Cargo changes are restricted to the public repository URL; the version remains
0.2.7. Deployment-helper changes are restricted to example-identifier sanitization.
Each actual exception requires a separate reason; stale/extra reason entries fail.
The helper does not grant approval merely by producing a candidate manifest.

Example operator sequence, with full reviewed commit SHAs and a reviewed external
JSON reason map supplied through environment variables:

```sh
# Use an existing real scratch path outside Git, without symlink ancestors.
python3 scripts/public_export.py propose --repo . --commit "$APPROVED_COMMIT" \
  --baseline "$PRIVATE_BASELINE" --changes "$REVIEWED_REASONS_JSON" \
  --approval "$SCRATCH/approval-candidate.json"
# Review candidate hashes, modes, differences and disclosure before export.
python3 scripts/public_export.py export --repo . --commit "$APPROVED_COMMIT" \
  --approval "$SCRATCH/approval-candidate.json" --output "$SCRATCH/public-tree" \
  --manifest "$SCRATCH/private-export-receipt.json"
python3 scripts/public_export.py verify --output "$SCRATCH/public-tree" \
  --manifest "$SCRATCH/private-export-receipt.json"
```

Keep candidate approvals and export receipts private: they contain the old/new
mapping and excluded-file inventory. The verifier requires an exact path/hash/mode
match and no Git metadata. The source destination already has one parentless clean root. After the exact
tree is accepted, a separate operator appends reviewed children to that root. Never copy `.git`, use a
mirror/fork/template/import, preserve old tags, or push old refs into the new source.
Initial approved branches resolve only to the clean root; later reviewed commits
may grow its history. Verify the root, tree, refs and downloaded source archive
again after the private push and before any public-visibility decision.

## Excluded material and disclosure checks

Raw authority/evidence, internal planning/review records, agent instructions,
requirements/matrices, local memory/graphs/caches, private configuration and
historical release/CI records are outside the literal public selection.
The original private records remain available in the private repository/backup.
Marker checks reject operator home paths, non-loopback IPv4 addresses and private
key blocks in selected content. They complement a separately reviewed credential
scan, exact public-tree review and Actions log/summary/artifact review. A passing
marker or secret-pattern scan is not proof that all confidential content is absent.

The legacy BAR register implementation remains byte-for-byte preserved. Hannes
Zietsman confirmed that the original C-derived implementation is his work and may
be published. Retain the active Rust backend and historical research credits;
the export helper must not silently remove production code. Final disclosure
review of the public tree, history, logs and artifacts remains separate.

The project already declares MIT and identifies CryptoLabs ZA in Cargo metadata.
`LICENSE` records that declared permission and author identity without inventing
a new copyright year. This is not independent proof of ownership of every historical
contribution. NVIDIA and Philip Langdale adaptation notices and pinned sources
remain in `docs/gpu-thermal-abi.md`; preserve them in redistribution. The public C
fixture is copied from the reviewed public-header assertion harness, with its
operator evidence/logs excluded. Third-party dependencies keep their own licenses.

## Trusted CI and artifact publication

Official candidates use the shared ephemeral VM pool. Only reviewed protected
`dev`/`main` pushes and manual dispatches from source1416116330 may reach the
trusted recipe. Public forks, PR heads and unreviewed refs remain ineligible.
The fully qualified caller pins a separately committed immutable recipe SHA;
Actions must stay held until that recipe and the exact selected group policy
have been admitted. No inherited organization secrets are part of the build.

The typed event repository identity and private/visibility fields select the
private staging lane or future public lane. Unknown/inconsistent metadata fails
closed. Hold Actions while changing visibility or routes, then verify effective
runner/workflow/ref/secret policy against live repository metadata. Neither a
label nor a workflow condition proves actual admission or runtime isolation.

The pinned guest-local Rust container runs as the ordinary guest UID with all
capabilities dropped, no privilege gains and explicit CPU/memory/PID limits.
It mounts only its disposable guest source directory. Host sockets, host keys,
GPU devices and management mounts are excluded. The retained strict native
builder is a separate path; its original guards have not been relaxed to fit
the generic guest. Actual guest/VM/disk/registration cleanup must be verified.

The driver-free exact590/595/615 ABI checks run before Cargo. Locked Linux
metadata and actual licence files produce complete dependency notices; unresolved
selected licensing fails the job. Retain the actual filtered metadata and
independently compare its non-dev closure to the emitted notice inventory.
Binary distributions include that generated notice and both DEBs carry exactly
those bytes as `/usr/share/doc/dc-exporter-rs/copyright`.

The distribution contract has ten ZIP entries, nine manifest assets and eight
checksum rows, with no manifest/self checksum. The exact source identity,
workflow/ref/SHA/run/attempt/artifact name and every permitted byte/hash must
match. Each file is bounded to128MiB and all entries total200MiB. Separate
notice-evidence artifacts support verification and are not release assets.

The trusted bytes-only publisher runs in its separately admitted ephemeral lane,
with source-only reader and release-only writer authority. It validates the
new-source contract and never executes downloaded binaries, source or package
hooks. Reject private-backup builds even if their names or bytes match. Keep
legacy legitimate release-asset recovery compatible across the schema change;
rehearse without writing public assets during preparation.

Actual admission denial checks, a source1416116330 shared run, independent artifact
inspection, cleanup and tree/log/artifact disclosure review remain prerequisites
for acceptance. Preserve the old private source/history and existing public
release assets. The owner confirmation above covers the original C-derived BAR
implementation; comprehensive disclosure review, verified private vulnerability
reporting and final owner visibility/asset approval remain separate gates.
Stable RTX5090 promotion requires accepted real-card qualification; software
checks and limited observed readings cannot replace it. Rollback disables only
the new candidate path and preserves other repositories, routes and pool limits.
