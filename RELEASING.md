# Releasing DC Exporter

## Branch and version policy

All development is committed on `dev`. Keep `Cargo.toml` at the existing version
while working on dev; the public-launch development baseline is **0.2.7**.
`main` is the release branch. Bump the version only as part of an approved
merge from dev to main, then tag that release using semantic versioning.
Do not copy historical private-source tags into the fresh source repository.

The initial standalone snapshot has no old parents, tags or fork/import history.
Maintain its private old-to-new commit mapping and exact file manifest separately.
Subsequent reviewed commits can extend the new history normally.

## Before a stable release

1. Review the exact tested tree, software results, license/provenance and release
   notes. A new stable RTX 5090 support claim requires accepted real-card evidence
   from [the lab procedure](docs/validation/rtx5090-acceptance.md).
2. Merge accepted dev work to main with the approved patch/minor/major version
   bump. Do not change dev's version during feature work.
3. Tag the accepted main commit. Verify that the trusted workflow, repository
   numeric identity, protected ref, commit and tag/version agree.
4. Build on the dedicated native ordinary-account runner, with the pinned
   toolchain and GLIBC 2.34 import ceiling. Retain the exact binary/package/hash
   manifest and the new-source workflow/run/artifact receipt.
5. Publish through the dedicated bytes-only publisher. It validates authoritative
   new-source identity, run/attempt, artifact identity, file allowlist and hashes.
   It never executes artifact binaries or package hooks with publication authority.
6. Verify the downloaded public bytes against the accepted artifact hashes.
   Review Actions logs, summaries and artifacts for disclosure before making
   fresh source history public.

Manual root build-server commands and credential-bearing staging are not part
of this release path. Public PR/fork heads do not run on either persistent VM.
A workflow label alone does not establish admission or isolation.

## Release destinations and holds

Source: [cryptolabsza/dc-exporter-rs-public](https://github.com/cryptolabsza/dc-exporter-rs-public).
Binaries and DEBs remain in
[cryptolabsza/dc-exporter-releases](https://github.com/cryptolabsza/dc-exporter-releases).
Development output uses `dev-latest` there. Historical stable assets keep their
original provenance; do not relabel private-backup artifacts as fresh-source builds.

The old source repository, history and CI records remain private. The new source
starts private with Actions held until effective secret/ref/runner controls and
rehearsals pass. Public visibility requires separate owner approval of the exact
tree and CI disclosures. Legacy-register disclosure/provenance and hardware
acceptance remain held. See [public publishing](docs/public-publishing.md).
