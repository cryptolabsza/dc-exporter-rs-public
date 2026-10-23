#!/usr/bin/env bash
# TRACEWEAVER: file-role=pool-build; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
set -euo pipefail
[[ "$(id -u)" != 0 ]]
[[ "$(uname -s)" == Linux ]]
[[ "$GITHUB_REPOSITORY" == cryptolabsza/dc-exporter-rs-public && "$GITHUB_REPOSITORY_ID" == 1416116330 ]]
[[ "$GITHUB_EVENT_NAME" == push || "$GITHUB_EVENT_NAME" == workflow_dispatch ]]
[[ "$GITHUB_REF_PROTECTED" == true ]]
[[ "$GITHUB_REF" == refs/heads/dev || "$GITHUB_REF" == refs/heads/main ]]
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]
# Check the typed event payload again in the guest before starting a container.
python3 scripts/ci/pool_manifest.py runner-group >/dev/null
[[ ! -e dist && ! -e build-evidence ]]
git ls-files --error-unmatch Cargo.lock >/dev/null
# Docker is guest-local. Only this disposable guest's source directory is mounted.
docker run --rm --platform linux/amd64 --cpus=2 --memory=3g --memory-swap=3g --pids-limit=256 \
  --user "$(id -u):$(id -g)" --cap-drop=ALL --security-opt=no-new-privileges \
  --mount "type=bind,source=$PWD,target=/build" --workdir /build \
  -e CARGO_HOME=/tmp/cargo -e CARGO_BUILD_JOBS=2 -e CARGO_TERM_COLOR=never \
  -e GITHUB_REPOSITORY -e GITHUB_REPOSITORY_ID -e GITHUB_SHA -e GITHUB_REF -e GITHUB_RUN_ID -e GITHUB_RUN_ATTEMPT \
  -e GITHUB_EVENT_NAME -e GITHUB_REF_PROTECTED \
  rust:1.88.0-bookworm@sha256:4727898c104ecd2e22d780925832502faee9fe4e70581b8572af081370b315a0 \
  bash -euo pipefail -c '
    rustc --version | grep -E "^rustc 1\.88\.0 "
    command -v cc
    python3 -m unittest discover -s tests -p "test_ci_*.py"
    python3 -m unittest discover -s tests -p "test_public_export.py"
    python3 -m unittest discover -s tests -p "test_pool_workflow.py"
    python3 scripts/ci/rusd_abi.py --cache /tmp/rusd-abi
    mkdir -p dist build-evidence
    cargo metadata --locked --format-version 1 --filter-platform x86_64-unknown-linux-gnu > build-evidence/cargo-metadata.json
    python3 scripts/ci/notice_inventory.py --metadata build-evidence/cargo-metadata.json --output dist/THIRD-PARTY-NOTICES.md --inventory build-evidence/notice-inventory.json
    cargo tree --locked --target x86_64-unknown-linux-gnu --edges normal,build --prefix none --format "{p}" > build-evidence/cargo-tree.txt
    cargo test --release --locked --all-targets -- --skip test_collector_creation --skip test_timeout_behavior
    cargo build --release --locked
    strip target/release/dc-exporter
    mkdir -p dist
    cp target/release/dc-exporter dist/dc-exporter-rs
    dist/dc-exporter-rs --version > dist/VERSION.txt
    dist/dc-exporter-rs --help > dist/HELP.txt
    grep -q -- --vram-backend dist/HELP.txt
    grep -q -- --tui dist/HELP.txt
    objdump -T dist/dc-exporter-rs > dist/SYMBOLS.txt
    python3 scripts/ci/build_contract.py glibc --symbols dist/SYMBOLS.txt
    version=$(python3 scripts/ci/build_contract.py version --binary-version "$(cat dist/VERSION.txt)")
    package=$(mktemp -d)
    trap '\''rm -rf -- "$package"'\'' EXIT
    mkdir -p "$package/DEBIAN" "$package/usr/local/bin" "$package/etc/systemd/system" "$package/usr/share/doc/dc-exporter-rs"
    cp dist/dc-exporter-rs "$package/usr/local/bin/dc-exporter-rs"
    printf "Package: dc-exporter-rs\nVersion: %s\nSection: utils\nPriority: optional\nArchitecture: amd64\nMaintainer: CryptoLabs ZA <admin@cryptolabs.co.za>\nDescription: GPU Metrics Exporter for Prometheus\n" "$version" > "$package/DEBIAN/control"
    cp systemd/dc-exporter.service "$package/etc/systemd/system/dc-exporter.service"
    cp dist/THIRD-PARTY-NOTICES.md "$package/usr/share/doc/dc-exporter-rs/copyright"
    chmod -R u=rwX,go=rX "$package"
    dpkg-deb --root-owner-group --build "$package" "dist/dc-exporter-rs_${version}_amd64.deb"
    cp "dist/dc-exporter-rs_${version}_amd64.deb" dist/dc-exporter-rs_amd64.deb
    printf "Repository: %s\nSource: %s\nRef: %s\nRun: %s\nAttempt: %s\nVersion: %s\n" "$GITHUB_REPOSITORY" "$GITHUB_SHA" "$GITHUB_REF" "$GITHUB_RUN_ID" "$GITHUB_RUN_ATTEMPT" "$version" > dist/BUILD_INFO.txt
    rustc --version >> dist/BUILD_INFO.txt
    cargo --version >> dist/BUILD_INFO.txt
    printf "Target: x86_64-unknown-linux-gnu\nFeatures: default\n" >> dist/BUILD_INFO.txt
    sha256sum build-evidence/notice-inventory.json >> dist/BUILD_INFO.txt
    (cd dist && sha256sum dc-exporter-rs *.deb BUILD_INFO.txt VERSION.txt HELP.txt SYMBOLS.txt THIRD-PARTY-NOTICES.md > SHA256SUMS)
    python3 scripts/ci/pool_manifest.py
  '
