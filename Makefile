# DC Exporter - Rust Build Makefile

BINARY_NAME = dc-exporter
TARGET_DIR = target/release
INSTALL_DIR = /usr/local/bin
CONFIG_DIR = /etc/dc-exporter

# Linux target for cross-compilation
LINUX_TARGET = x86_64-unknown-linux-gnu

.PHONY: all build release clean install uninstall test lint fmt cross-linux deploy

all: release

# Development build
build:
	cargo build

# Release build (optimized)
release:
	cargo build --release
	@echo "Binary: $(TARGET_DIR)/$(BINARY_NAME)"
	@ls -lh $(TARGET_DIR)/$(BINARY_NAME)

# Clean build artifacts
clean:
	cargo clean

# Run tests
test:
	cargo test

# Lint with clippy
lint:
	cargo clippy -- -D warnings

# Format code
fmt:
	cargo fmt

# Cross-compile for Linux x86_64
cross-linux:
	@echo "Cross-compiling for Linux x86_64..."
	@echo "Note: Requires 'cross' tool: cargo install cross"
	cross build --release --target $(LINUX_TARGET)
	@ls -lh target/$(LINUX_TARGET)/release/$(BINARY_NAME)

# Install locally (requires sudo)
install: release
	sudo cp $(TARGET_DIR)/$(BINARY_NAME) $(INSTALL_DIR)/
	sudo chmod +x $(INSTALL_DIR)/$(BINARY_NAME)
	sudo mkdir -p $(CONFIG_DIR)
	@if [ ! -f $(CONFIG_DIR)/config.toml ]; then \
		sudo cp config.toml.example $(CONFIG_DIR)/config.toml; \
	fi
	@echo ""
	@echo "Installed $(BINARY_NAME) to $(INSTALL_DIR)"
	@echo "Config at $(CONFIG_DIR)/config.toml"

# Uninstall
uninstall:
	sudo rm -f $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "Removed $(INSTALL_DIR)/$(BINARY_NAME)"

# Deploy to remote server
# Usage: make deploy HOST=user@server
deploy: release
	@if [ -z "$(HOST)" ]; then \
		echo "Usage: make deploy HOST=user@server"; \
		exit 1; \
	fi
	@echo "Deploying to $(HOST)..."
	scp $(TARGET_DIR)/$(BINARY_NAME) $(HOST):/tmp/
	scp config.toml.example $(HOST):/tmp/config.toml
	ssh $(HOST) "sudo mv /tmp/$(BINARY_NAME) $(INSTALL_DIR)/ && \
		sudo chmod +x $(INSTALL_DIR)/$(BINARY_NAME) && \
		sudo mkdir -p $(CONFIG_DIR) && \
		[ ! -f $(CONFIG_DIR)/config.toml ] && sudo mv /tmp/config.toml $(CONFIG_DIR)/ || rm /tmp/config.toml && \
		sudo systemctl restart dc-exporter 2>/dev/null || true"
	@echo "Deployed to $(HOST)"

# Create systemd service file
systemd:
	@echo "[Unit]" > dc-exporter.service
	@echo "Description=DC Exporter - GPU Metrics (Rust)" >> dc-exporter.service
	@echo "After=network.target nvidia-persistenced.service" >> dc-exporter.service
	@echo "" >> dc-exporter.service
	@echo "[Service]" >> dc-exporter.service
	@echo "Type=simple" >> dc-exporter.service
	@echo "ExecStart=/usr/local/bin/dc-exporter --port 9835" >> dc-exporter.service
	@echo "Restart=always" >> dc-exporter.service
	@echo "RestartSec=5" >> dc-exporter.service
	@echo "" >> dc-exporter.service
	@echo "[Install]" >> dc-exporter.service
	@echo "WantedBy=multi-user.target" >> dc-exporter.service
	@echo "Created dc-exporter.service"

# Full install with systemd
install-full: install systemd
	sudo cp dc-exporter.service /etc/systemd/system/
	sudo systemctl daemon-reload
	sudo systemctl enable dc-exporter
	sudo systemctl start dc-exporter
	@echo ""
	@echo "DC Exporter installed and started!"
	@echo "Check status: sudo systemctl status dc-exporter"
	@echo "View logs: sudo journalctl -u dc-exporter -f"
	@echo "Test: curl http://localhost:9835/metrics | grep DCXP"
