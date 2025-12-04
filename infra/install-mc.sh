#!/bin/sh
# Download MinIO Client (mc) binary for the appropriate architecture
# This script is run during Docker build

set -e

ARCH=$(uname -m)

echo "[BUILD] Downloading MinIO Client (mc) for ${ARCH}..."

case "$ARCH" in
    x86_64)
        MC_ARCH="linux-amd64"
        ;;
    aarch64|arm64)
        MC_ARCH="linux-arm64"
        ;;
    *)
        echo "[BUILD] Unsupported architecture: $ARCH"
        echo "[BUILD] MinIO Client will not be available"
        exit 1
        ;;
esac

DOWNLOAD_URL="https://dl.min.io/client/mc/release/${MC_ARCH}/mc"

echo "[BUILD] Downloading from: $DOWNLOAD_URL"

# Download with retry
MAX_RETRIES=3
RETRY_COUNT=0

while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if wget -O /tmp/mc "$DOWNLOAD_URL" 2>/dev/null; then
        echo "[BUILD] ✓ Download successful"
        break
    fi
    
    RETRY_COUNT=$((RETRY_COUNT + 1))
    echo "[BUILD] Download failed, retry $RETRY_COUNT/$MAX_RETRIES..."
    sleep 2
done

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo "[BUILD] ✗ Failed to download MinIO Client after $MAX_RETRIES attempts"
    exit 1
fi

# Install binary
chmod +x /tmp/mc
mv /tmp/mc /usr/local/bin/mc

echo "[BUILD] ✓ MinIO Client (mc) installed successfully"
/usr/local/bin/mc --version

exit 0