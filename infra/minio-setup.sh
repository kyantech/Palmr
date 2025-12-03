#!/bin/sh
# Storage System Automatic Setup Script
# This script automatically configures storage system on first boot
# No user intervention required

set -e

# Configuration
MINIO_DATA_DIR="${MINIO_DATA_DIR:-/app/server/minio-data}"
MINIO_ROOT_USER="palmr-minio-admin"
MINIO_ROOT_PASSWORD="$(cat /app/server/.minio-root-password 2>/dev/null || echo 'password-not-generated')"
MINIO_BUCKET="${MINIO_BUCKET:-palmr-files}"
MINIO_INITIALIZED_FLAG="/app/server/.minio-initialized"
MINIO_CREDENTIALS="/app/server/.minio-credentials"

echo "[STORAGE-SYSTEM-SETUP] Starting storage system configuration..."

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

# Configure storage client (mc) - ALWAYS reconfigure with current password
echo "[STORAGE-SYSTEM-SETUP] Configuring storage client..."
mc alias set palmr-local http://127.0.0.1:9379 "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" 2>/dev/null || {
    echo "[STORAGE-SYSTEM-SETUP] ✗ Failed to configure storage client"
    exit 1
}

# Create bucket (idempotent - won't fail if exists)
echo "[STORAGE-SYSTEM-SETUP] Ensuring storage bucket exists: $MINIO_BUCKET..."
if mc ls palmr-local/$MINIO_BUCKET > /dev/null 2>&1; then
    echo "[STORAGE-SYSTEM-SETUP]   ✓ Bucket '$MINIO_BUCKET' already exists"
else
    echo "[STORAGE-SYSTEM-SETUP]   Creating bucket '$MINIO_BUCKET'..."
    mc mb "palmr-local/$MINIO_BUCKET" 2>/dev/null || {
        echo "[STORAGE-SYSTEM-SETUP] ✗ Failed to create bucket"
        exit 1
    }
    echo "[STORAGE-SYSTEM-SETUP]   ✓ Bucket created"
fi

# Set bucket policy to private (always reapply)
echo "[STORAGE-SYSTEM-SETUP] Setting bucket policy..."
mc anonymous set none "palmr-local/$MINIO_BUCKET" 2>/dev/null || true

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

