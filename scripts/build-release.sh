#!/bin/bash
# Build release packages for dc-exporter
set -e

VERSION="${1:-3.0.0}"
ARCH="x86_64"
PKG_NAME="dc-exporter"
BUILD_DIR="target/release"
DIST_DIR="dist"

echo "Building dc-exporter v${VERSION} for Linux ${ARCH}"

# Clean and create dist directory
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}"

# Build release binary
echo "==> Building release binary..."
cargo build --release

# Strip the binary to reduce size
echo "==> Stripping binary..."
strip "${BUILD_DIR}/${PKG_NAME}" || true

# Get binary size
BINARY_SIZE=$(du -h "${BUILD_DIR}/${PKG_NAME}" | cut -f1)
echo "Binary size: ${BINARY_SIZE}"

# Create tarball
echo "==> Creating tarball..."
TARBALL="${PKG_NAME}-${VERSION}-linux-${ARCH}.tar.gz"
mkdir -p "${DIST_DIR}/tarball/${PKG_NAME}"
cp "${BUILD_DIR}/${PKG_NAME}" "${DIST_DIR}/tarball/${PKG_NAME}/"
cp README.md "${DIST_DIR}/tarball/${PKG_NAME}/" 2>/dev/null || true
cp config.toml.example "${DIST_DIR}/tarball/${PKG_NAME}/" 2>/dev/null || true
cp -r systemd "${DIST_DIR}/tarball/${PKG_NAME}/" 2>/dev/null || true
tar -czf "${DIST_DIR}/${TARBALL}" -C "${DIST_DIR}/tarball" "${PKG_NAME}"
rm -rf "${DIST_DIR}/tarball"

# Create DEB package
echo "==> Creating DEB package..."
DEB_DIR="${DIST_DIR}/deb"
DEB_PKG="${PKG_NAME}_${VERSION}_amd64.deb"

mkdir -p "${DEB_DIR}/DEBIAN"
mkdir -p "${DEB_DIR}/usr/local/bin"
mkdir -p "${DEB_DIR}/etc/dc-exporter"
mkdir -p "${DEB_DIR}/lib/systemd/system"

# Copy binary
cp "${BUILD_DIR}/${PKG_NAME}" "${DEB_DIR}/usr/local/bin/"

# Copy config
cp config.toml.example "${DEB_DIR}/etc/dc-exporter/config.toml" 2>/dev/null || true

# Copy systemd service
if [ -f "systemd/dc-exporter.service" ]; then
    cp "systemd/dc-exporter.service" "${DEB_DIR}/lib/systemd/system/"
fi

# Create control file
cat > "${DEB_DIR}/DEBIAN/control" << EOF
Package: ${PKG_NAME}
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Maintainer: CrypotAI Team
Description: DC Exporter - GPU Metrics Exporter for Prometheus
 Collects GPU metrics from NVIDIA GPUs and exposes them in Prometheus format.
 Features:
  - Safe NVML access with timeout protection
  - VM passthrough detection via PCIe scanning
  - Direct register access for VRAM/Hotspot temperatures
  - DCGM-compatible metrics
  - TUI mode for interactive monitoring
EOF

# Create postinst script
cat > "${DEB_DIR}/DEBIAN/postinst" << 'EOF'
#!/bin/bash
set -e
systemctl daemon-reload
echo "DC Exporter installed. To start:"
echo "  systemctl enable dc-exporter"
echo "  systemctl start dc-exporter"
EOF
chmod 755 "${DEB_DIR}/DEBIAN/postinst"

# Create prerm script
cat > "${DEB_DIR}/DEBIAN/prerm" << 'EOF'
#!/bin/bash
set -e
systemctl stop dc-exporter 2>/dev/null || true
systemctl disable dc-exporter 2>/dev/null || true
EOF
chmod 755 "${DEB_DIR}/DEBIAN/prerm"

# Build DEB
dpkg-deb --build "${DEB_DIR}" "${DIST_DIR}/${DEB_PKG}" 2>/dev/null || {
    echo "dpkg-deb not available, skipping DEB creation"
}
rm -rf "${DEB_DIR}"

# Create checksums
echo "==> Creating checksums..."
cd "${DIST_DIR}"
sha256sum * > SHA256SUMS 2>/dev/null || shasum -a 256 * > SHA256SUMS
cd ..

echo ""
echo "==> Release packages created in ${DIST_DIR}/"
ls -la "${DIST_DIR}/"

echo ""
echo "Done! Version ${VERSION} packages ready for release."
