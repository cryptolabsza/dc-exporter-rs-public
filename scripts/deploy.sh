#!/bin/bash
# Deploy dc-exporter to remote servers
# Usage: ./scripts/deploy.sh [user@]host[:port] [binary_path]
#
# Examples:
#   ./scripts/deploy.sh root@lab.example.invalid
#   ./scripts/deploy.sh root@server.com:2222
#   ./scripts/deploy.sh root@server.com /path/to/dc-exporter

set -e

# Parse arguments
TARGET="$1"
BINARY="${2:-}"
SSH_KEY="${SSH_KEY:-~/.ssh/ubuntu_key}"

if [ -z "$TARGET" ]; then
    echo "Usage: $0 [user@]host[:port] [binary_path]"
    echo ""
    echo "Environment variables:"
    echo "  SSH_KEY    - Path to SSH key (default: ~/.ssh/ubuntu_key)"
    echo ""
    echo "Examples:"
    echo "  $0 root@lab.example.invalid"
    echo "  $0 root@server.com:2222"
    echo "  SSH_KEY=~/.ssh/id_rsa $0 user@host"
    exit 1
fi

# Parse target into user, host, port
if [[ "$TARGET" =~ ^([^@]+)@([^:]+):?([0-9]*)$ ]]; then
    USER="${BASH_REMATCH[1]}"
    HOST="${BASH_REMATCH[2]}"
    PORT="${BASH_REMATCH[3]:-22}"
elif [[ "$TARGET" =~ ^([^:]+):?([0-9]*)$ ]]; then
    USER="root"
    HOST="${BASH_REMATCH[1]}"
    PORT="${BASH_REMATCH[2]:-22}"
else
    echo "Invalid target format: $TARGET"
    exit 1
fi

echo "Deploying to ${USER}@${HOST}:${PORT}"

# Find binary
if [ -n "$BINARY" ] && [ -f "$BINARY" ]; then
    BINARY_PATH="$BINARY"
elif [ -f "dist/dc-exporter-linux-x86_64" ]; then
    BINARY_PATH="dist/dc-exporter-linux-x86_64"
elif [ -f "target/x86_64-unknown-linux-musl/release/dc-exporter" ]; then
    BINARY_PATH="target/x86_64-unknown-linux-musl/release/dc-exporter"
elif [ -f "target/release/dc-exporter" ]; then
    BINARY_PATH="target/release/dc-exporter"
else
    echo "No binary found. Build first with: cargo build --release"
    echo "Or download from GitHub releases."
    exit 1
fi

echo "Using binary: $BINARY_PATH"

# Expand SSH_KEY path
SSH_KEY="${SSH_KEY/#\~/$HOME}"

SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=10"
if [ -f "$SSH_KEY" ]; then
    SSH_OPTS="$SSH_OPTS -i $SSH_KEY"
fi

# Deploy
echo "==> Copying binary..."
scp $SSH_OPTS -P "$PORT" "$BINARY_PATH" "${USER}@${HOST}:/tmp/dc-exporter"

echo "==> Installing..."
ssh $SSH_OPTS -p "$PORT" "${USER}@${HOST}" "
    chmod +x /tmp/dc-exporter
    
    # Test it works
    echo '==> Testing binary...'
    /tmp/dc-exporter --one-shot 2>&1 | head -20
    
    # Install to /usr/local/bin
    echo '==> Installing to /usr/local/bin...'
    mv /tmp/dc-exporter /usr/local/bin/dc-exporter
    
    # Show version
    /usr/local/bin/dc-exporter --help | head -5
"

echo ""
echo "==> Deployment complete!"
echo "Run: ssh -p $PORT ${USER}@${HOST} 'dc-exporter --one-shot'"
