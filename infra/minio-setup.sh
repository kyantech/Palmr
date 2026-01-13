#!/bin/sh
# Storage System Automatic Setup Script
# This script automatically configures storage system on first boot
# No user intervention required

set -e

# Skip internal storage setup if external S3 is enabled
if [ "$ENABLE_S3" = "true" ]; then
  echo "[STORAGE-SYSTEM-SETUP] External S3 enabled (ENABLE_S3=true)"
  echo "[STORAGE-SYSTEM-SETUP] Skipping internal storage setup"
  exit 0
fi

# Configuration
MINIO_DATA_DIR="${MINIO_DATA_DIR:-/app/server/minio-data}"
MINIO_ROOT_USER="palmr-minio-admin"
MINIO_ROOT_PASSWORD="$(cat /app/server/.minio-root-password 2>/dev/null || echo 'password-not-generated')"
MINIO_BUCKET="${MINIO_BUCKET:-palmr-files}"
MINIO_INITIALIZED_FLAG="/app/server/.minio-initialized"
MINIO_CREDENTIALS="/app/server/.minio-credentials"

echo "[STORAGE-SYSTEM-SETUP] Starting internal storage system configuration..."

# Create data directory
mkdir -p "$MINIO_DATA_DIR"

# Wait for storage system to start (managed by supervisor)
echo "[STORAGE-SYSTEM-SETUP] Waiting for storage system to start..."
MAX_RETRIES=30
RETRY_COUNT=0

while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if curl -sf http://127.0.0.1:9379/minio/health/live > /dev/null 2>&1; then
        echo "[STORAGE-SYSTEM-SETUP]   ✓ Storage system is responding"
        break
    fi
    RETRY_COUNT=$((RETRY_COUNT + 1))
    echo "[STORAGE-SYSTEM-SETUP]   Waiting... ($RETRY_COUNT/$MAX_RETRIES)"
    sleep 2
done

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo "[STORAGE-SYSTEM-SETUP] ✗ Storage system failed to start"
    exit 1
fi

# Configure storage client (mc) - Run as target UID/GID
echo "[STORAGE-SYSTEM-SETUP] Configuring storage client..."

# Get target UID/GID from environment or default
TARGET_UID=${PALMR_UID:-${MINIO_UID:-1001}}
TARGET_GID=${PALMR_GID:-${MINIO_GID:-1001}}

# Ensure home directory exists with correct ownership
if [ "$(id -u)" = "0" ]; then
    mkdir -p /home/palmr/.mc
    chown -R $TARGET_UID:$TARGET_GID /home/palmr 2>/dev/null || true
fi

# Run mc commands as target user with explicit config location
run_as_target() {
    if [ "$(id -u)" = "0" ]; then
        su-exec $TARGET_UID:$TARGET_GID \
            env HOME="/home/palmr" MC_CONFIG_DIR="/home/palmr/.mc" \
            "$@"
    else
        HOME="/home/palmr" MC_CONFIG_DIR="/home/palmr/.mc" "$@"
    fi
}

# Configure with verbose error output
if ! run_as_target mc alias set palmr-local http://127.0.0.1:9379 "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" 2>&1; then
    echo "[STORAGE-SYSTEM-SETUP] ✗ Failed to configure storage client"
    echo "[STORAGE-SYSTEM-SETUP] Debug: UID/GID=$TARGET_UID:$TARGET_GID, User=$(whoami)"
    exit 1
fi

# Create bucket (idempotent - won't fail if exists)
echo "[STORAGE-SYSTEM-SETUP] Ensuring storage bucket exists: $MINIO_BUCKET..."
if run_as_target mc ls palmr-local/$MINIO_BUCKET > /dev/null 2>&1; then
    echo "[STORAGE-SYSTEM-SETUP]   ✓ Bucket '$MINIO_BUCKET' already exists"
else
    echo "[STORAGE-SYSTEM-SETUP]   Creating bucket '$MINIO_BUCKET'..."
    run_as_target mc mb "palmr-local/$MINIO_BUCKET" 2>/dev/null || {
        echo "[STORAGE-SYSTEM-SETUP] ✗ Failed to create bucket"
        exit 1
    }
    echo "[STORAGE-SYSTEM-SETUP]   ✓ Bucket created"
fi

# Set bucket policy to private (always reapply)
echo "[STORAGE-SYSTEM-SETUP] Setting bucket policy..."
run_as_target mc anonymous set none "palmr-local/$MINIO_BUCKET" 2>/dev/null || true

# Save credentials for Palmr to use
echo "[STORAGE-SYSTEM-SETUP] Saving credentials to $MINIO_CREDENTIALS..."

# Create credentials file
cat > "$MINIO_CREDENTIALS" <<EOF
S3_ENDPOINT=127.0.0.1
S3_PORT=9379
S3_ACCESS_KEY=$MINIO_ROOT_USER
S3_SECRET_KEY=$MINIO_ROOT_PASSWORD
S3_BUCKET_NAME=$MINIO_BUCKET
S3_REGION=us-east-1
S3_USE_SSL=false
S3_FORCE_PATH_STYLE=true
EOF

# Verify file was created
if [ ! -f "$MINIO_CREDENTIALS" ]; then
    echo "[STORAGE-SYSTEM-SETUP] ✗ ERROR: Failed to create credentials file!"
    echo "[STORAGE-SYSTEM-SETUP] Check permissions on /app/server directory"
    exit 1
fi

chmod 644 "$MINIO_CREDENTIALS" 2>/dev/null || true
echo "[STORAGE-SYSTEM-SETUP] ✓ Credentials file created and readable"

echo "[STORAGE-SYSTEM-SETUP] ✓✓✓ Storage system configured successfully!"
echo "[STORAGE-SYSTEM-SETUP]   Bucket: $MINIO_BUCKET"
echo "[STORAGE-SYSTEM-SETUP]   Credentials: saved to .minio-credentials"
echo "[STORAGE-SYSTEM-SETUP]   Palmr will use storage system"

exit 0

