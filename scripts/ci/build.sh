#!/usr/bin/env bash
set -euo pipefail
# TRACEWEAVER: file-role=native-ci-build-entrypoint; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
umask 077
/usr/bin/python3 scripts/ci/build_contract.py preflight
git ls-files --error-unmatch Cargo.lock > /dev/null
/usr/bin/python3 scripts/ci/build_contract.py lock
[[ "$GITHUB_REPOSITORY" == cryptolabsza/dc-exporter-rs-public && "$GITHUB_REPOSITORY_ID" == 1416116330 ]]
[[ "$GITHUB_REF" == refs/heads/dev || "$GITHUB_REF" == refs/heads/main ]]
[[ "$GITHUB_RUN_ID" =~ ^[1-9][0-9]*$ && "$GITHUB_RUN_ATTEMPT" =~ ^[1-9][0-9]*$ && "$GITHUB_SHA" =~ ^[0-9a-f]{40}$ ]]
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" ]]
[[ "$(rustc --version)" == 'rustc 1.88.0 '* ]]
work="$(mktemp -d "$RUNNER_TEMP/dc-exporter-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}.XXXXXX")"
success=false
# TRACEWEAVER: entrypoint=cleanup; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
cleanup() {
  rm -rf -- "$work/target" "$work/cargo" "$work/package"
  if [[ "$success" != true ]]; then rm -rf -- "$work"; fi
}
trap cleanup EXIT
export CARGO_HOME="$work/cargo" CARGO_TARGET_DIR="$work/target" CARGO_BUILD_JOBS=2 RUSTUP_TOOLCHAIN=1.88.0
ulimit -t 1200
ulimit -v 6291456
ulimit -u 256
ulimit -f 1048576
/usr/bin/python3 -m unittest discover -s tests -p 'test_ci_*.py'
/usr/bin/python3 -m unittest discover -s tests -p 'test_public_export.py'
cargo test --release --locked --all-targets -- --skip test_collector_creation --skip test_timeout_behavior
cargo build --release --locked
strip "$CARGO_TARGET_DIR/release/dc-exporter"
objdump -T "$CARGO_TARGET_DIR/release/dc-exporter" > "$work/symbols.txt"
/usr/bin/python3 scripts/ci/build_contract.py glibc --symbols "$work/symbols.txt"
binary_version="$("$CARGO_TARGET_DIR/release/dc-exporter" --version)"
version="$(/usr/bin/python3 scripts/ci/build_contract.py version --binary-version "$binary_version")"
"$CARGO_TARGET_DIR/release/dc-exporter" --help > "$work/help.txt"
grep -q -- '--vram-backend' "$work/help.txt"
mkdir "$work/dist"
cp "$CARGO_TARGET_DIR/release/dc-exporter" "$work/dist/dc-exporter-rs"
chmod 755 "$work/dist/dc-exporter-rs"
package="$work/package"
mkdir -p "$package/DEBIAN" "$package/usr/local/bin" "$package/etc/systemd/system"
chmod 755 "$package" "$package/DEBIAN" "$package/usr" "$package/usr/local" "$package/usr/local/bin" "$package/etc" "$package/etc/systemd" "$package/etc/systemd/system"
cp "$work/dist/dc-exporter-rs" "$package/usr/local/bin/dc-exporter-rs"
cat > "$package/DEBIAN/control" <<CONTROL
Package: dc-exporter-rs
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: CryptoLabs ZA <admin@cryptolabs.co.za>
Description: GPU Metrics Exporter for Prometheus
CONTROL
cat > "$package/etc/systemd/system/dc-exporter.service" <<'SERVICE'
[Unit]
Description=DC Exporter - GPU Metrics for Prometheus
After=network.target nvidia-persistenced.service
[Service]
Type=simple
ExecStart=/usr/local/bin/dc-exporter-rs --port 9835
Restart=always
RestartSec=5
[Install]
WantedBy=multi-user.target
SERVICE
chmod 644 "$package/DEBIAN/control" "$package/etc/systemd/system/dc-exporter.service"
# Packaging reads bytes. No maintainer scripts are created or executed.
dpkg-deb --root-owner-group --build "$package" "$work/dist/dc-exporter-rs_${version}_amd64.deb" > "$work/package.log"
cp "$work/dist/dc-exporter-rs_${version}_amd64.deb" "$work/dist/dc-exporter-rs_amd64.deb"
printf 'Repository: %s\nSource: %s\nRef: %s\nRun: %s\nAttempt: %s\nVersion: %s\n' "$GITHUB_REPOSITORY" "$GITHUB_SHA" "$GITHUB_REF" "$GITHUB_RUN_ID" "$GITHUB_RUN_ATTEMPT" "$version" > "$work/dist/BUILD_INFO.txt"
(cd "$work/dist" && sha256sum dc-exporter-rs dc-exporter-rs_amd64.deb "dc-exporter-rs_${version}_amd64.deb" BUILD_INFO.txt > SHA256SUMS)
/usr/bin/python3 scripts/ci/build_contract.py manifest --dist "$work/dist" --cargo-toml Cargo.toml --binary-version "$version"
printf 'artifact_dir=%s\n' "$work/dist" >> "$GITHUB_OUTPUT"
success=true
