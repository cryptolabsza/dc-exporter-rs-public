# Releasing CryptoLabs DC Exporter

## Branch and version policy

Development changes go to `dev`; keep version **0.2.7** during this preparation.
An accepted merge to release branch `main` carries the separately approved
semantic version bump and release tag. The trusted build runs from the protected
branch commit, not from an arbitrary tag or contribution head.

The source repository has a clean standalone root. Reviewed child commits extend
that history; never push old private parents, tags or refs into it. Retain the
old-to-new source mapping and exact export approval privately. Historical stable
0.2.8 assets keep their original provenance and are not builds of this fresh tree.

## Build and verify

1. Review the exact protected source commit, immutable caller/recipe pin, software
   results, notices and provenance. Public PR/fork/unreviewed refs are ineligible.
2. Build on the shared ephemeral runner pool using the pinned Rust container,
   locked dependencies, driver-free ABI assertions before Cargo and GLIBC 2.34
   import ceiling. Preserve source ID `1416116330`, SHA, run and attempt.
3. Verify the ten-entry distribution ZIP: executable, two DEBs, `BUILD_INFO.txt`,
   `VERSION.txt`, `HELP.txt`, `SYMBOLS.txt`, `THIRD-PARTY-NOTICES.md`, `SHA256SUMS`
   and `manifest.json`. The manifest covers nine assets and checksums cover eight.
4. Independently compare the locked Linux non-dev graph to the notice inventory;
   inspect actual licence texts and exact notice equality in both DEBs at
   `/usr/share/doc/dc-exporter-rs/copyright`. Missing licensing blocks distribution.
   The packaged executable is `/usr/local/bin/dc-exporter-rs`; the maintained
   `dc-exporter.service` starts it on port9835.
5. Use the trusted bytes-only publisher in its scoped ephemeral lane. It checks
   authoritative new-source workflow/ref/SHA/run/attempt/artifact identity, strict
   names/sizes/hashes and the agreed schema. It never executes downloaded binaries,
   package hooks or source code with publication authority. Retained private-source
   builds remain invalid even when their filenames or hashes happen to match.
6. Rehearse validation and recovery without replacing public assets. Recovery must
   restore prior legitimate smaller asset sets byte-for-byte across the schema
   transition. Compare any later public downloads to the exact accepted hashes.

## Admission and publication gates

The same source recipe selects the private staging lane while the repository is
private and the public ephemeral lane after an explicit visibility decision.
The event's typed repository identity/private/visibility fields select that lane;
unknown or inconsistent payloads fail closed. Effective group/workflow selection,
protected refs, secret access and runtime boundaries require live verification.
Hold Actions during any visibility/routing transition and verify those controls
before allowing new work. A YAML condition or label alone is not admission.

Source: [dc-exporter-rs-public](https://github.com/cryptolabsza/dc-exporter-rs-public).
Downloads: [dc-exporter-releases](https://github.com/cryptolabsza/dc-exporter-releases/releases).
Development output uses `dev-latest`; the original private repository/history
remain private. Existing public assets remain unchanged during this preparation.

The owner must accept the exact tree, Actions logs/summaries/artifacts, complete
notices, legacy BAR provenance/disclosure and private security-reporting readiness
before public source visibility. Public asset replacement requires its own final
approval. Stable RTX5090 promotion additionally requires accepted actual-card
accuracy, GPU Burn/NCCL and noninterference evidence. Exact ABI profiles and
limited functional readings do not establish that qualification.

Rollback disables only this candidate's new workflow/admission/publication path;
preserve private history, old public assets, unrelated pool routes and capacity.
See [public publishing](docs/public-publishing.md) and [testing](TESTING.md).
