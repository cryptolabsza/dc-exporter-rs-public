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
match and no Git metadata. After the exact tree is accepted, a separate operator
creates one parentless root in a standalone repository. Never copy `.git`, use a
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

The legacy BAR register implementation remains byte-for-byte preserved. Its
historical provenance and confidentiality are not established by the RUSD public
source notices. Owner clearance of that legacy disclosure remains required before
public visibility; the export helper must not silently remove production code.

The project already declares MIT and identifies CryptoLabs ZA in Cargo metadata.
`LICENSE` records that declared permission and author identity without inventing
a new copyright year. This is not independent proof of ownership of every historical
contribution. NVIDIA and Philip Langdale adaptation notices and pinned sources
remain in `docs/gpu-thermal-abi.md`; preserve them in redistribution. The public C
fixture is copied from the reviewed public-header assertion harness, with its
operator evidence/logs excluded. Third-party dependencies keep their own licenses.

## Trusted CI and artifact publication

Public PR/fork/unreviewed-ref code does not execute on either persistent VM.
Reviewed protected dev/main refs are the only trusted build inputs. Dedicated
ordinary-account native build/publisher runners require verified selected
repository/workflow/ref admission, no sudo/rootful socket/admin mounts and no
unrelated organization secrets. Build authority is read-only; publication authority
is target-only and scoped to the release repository. Labels/YAML conditions alone
cannot prove these control-plane or runtime boundaries.

The bytes-only publisher admits the new source numeric ID and exact authoritative
workflow/run/attempt/ref/commit/artifact identity, validates safe archive entries
and exact file/hash manifests, and rejects retained-private-source runs even when
names or SHAs match. It does not execute artifact code or package hooks. Do not
reuse shared executable caches or relabel old-source runs as new-source evidence.
Recheck effective policies after any visibility change, with listeners held.
The hypervisor remains a trusted boundary; privileged workloads elsewhere on it
are not repaired by this pipeline's isolation.

Only after exact software/artifact checks, admission denial tests, runtime inspection
and rehearsal disclosure review may the owner approve public source visibility.
Stable RTX 5090 promotion also requires actual accepted lab evidence. Recovery
preserves the private backup and disables only this exporter's new publication path;
never restore old history/private records into the public repository.
