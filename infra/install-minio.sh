#!/bin/sh
# Download storage system binary for the appropriate architecture
# This script is run during Docker build

set -e

MINIO_VERSION="RELEASE.2024-10-13T13-34-11Z"
ARCH=$(uname -m)

echo "[BUILD] Downloading storage system ${MINIO_VERSION} for ${ARCH}..."

case "$ARCH" in
    x86_64)
        MINIO_ARCH="linux-amd64"
        ;;
    aarch64|arm64)
        MINIO_ARCH="linux-arm64"
        ;;
    *)
        echo "[BUILD] Unsupported architecture: $ARCH"
        echo "[BUILD] Palmr will fallback to external S3"
        exit 0
        ;;
esac

DOWNLOAD_URL="https://dl.min.io/server/minio/release/${MINIO_ARCH}/archive/minio.${MINIO_VERSION}"

echo "[BUILD] Downloading from: $DOWNLOAD_URL"

# Download with retry
MAX_RETRIES=3
RETRY_COUNT=0

while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if wget -O /tmp/minio "$DOWNLOAD_URL" 2>/dev/null; then
        echo "[BUILD] ✓ Download successful"
        break
    fi
    
    RETRY_COUNT=$((RETRY_COUNT + 1))
    echo "[BUILD] Download failed, retry $RETRY_COUNT/$MAX_RETRIES..."
    sleep 2
done

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo "[BUILD] ✗ Failed to download storage system after $MAX_RETRIES attempts"
    echo "[BUILD] Palmr will fallback to external S3"
    exit 0
fi

# Install binary
chmod +x /tmp/minio
mv /tmp/minio /usr/local/bin/minio

echo "[BUILD] ✓ Storage system installed successfully"
/usr/local/bin/minio --version

exit 0


