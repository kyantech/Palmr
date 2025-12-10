#!/bin/sh
# Start storage system with persistent root password
# This script MUST run as root to fix permissions, then drops to palmr user

set -e

# Skip internal storage if external S3 is enabled
if [ "$ENABLE_S3" = "true" ]; then
  echo "[STORAGE-SYSTEM] External S3 enabled (ENABLE_S3=true)"
  echo "[STORAGE-SYSTEM] Skipping internal storage system"
  # Keep supervisor happy by sleeping forever instead of exiting
  # This prevents supervisor from trying to restart the process
  # Alternative would be to dynamically generate supervisord.conf, but this is simpler
  exec tail -f /dev/null
fi

DATA_DIR="/app/server/minio-data"
PASSWORD_FILE="/app/server/.minio-root-password"
MINIO_USER="palmr"

echo "[STORAGE-SYSTEM] Initializing internal storage..."

# USE ENVIRONMENT VARIABLES: Allow runtime UID/GID configuration
# Falls back to palmr user's UID/GID if not specified
export MINIO_UID=${PALMR_UID:-$(id -u $MINIO_USER 2>/dev/null || echo "1001")}
export MINIO_GID=${PALMR_GID:-$(id -g $MINIO_USER 2>/dev/null || echo "1001")}

echo "[STORAGE-SYSTEM]   Target user: $MINIO_USER (UID:$MINIO_UID, GID:$MINIO_GID)"

# Ensure directory exists and has correct permissions
if [ "$(id -u)" = "0" ]; then
    mkdir -p "$DATA_DIR"
    
    # Clean metadata if exists
    if [ -d "$DATA_DIR/.minio.sys" ]; then
        echo "[STORAGE-SYSTEM]   Cleaning metadata..."
        rm -rf "$DATA_DIR/.minio.sys" 2>/dev/null || true
    fi
    
    # CRITICAL: MinIO needs write access to ALL subdirectories for multipart uploads
    # Check if ownership is correct before doing expensive recursive chown
    CURRENT_OWNER=$(stat -c '%u:%g' "$DATA_DIR" 2>/dev/null || stat -f '%u:%g' "$DATA_DIR" 2>/dev/null || echo "0:0")
    TARGET_OWNER="${MINIO_UID}:${MINIO_GID}"
    
    if [ "$CURRENT_OWNER" != "$TARGET_OWNER" ] || [ -n "$(find "$DATA_DIR" -type f -o -type d ! -user ${MINIO_UID} 2>/dev/null | head -1)" ]; then
        echo "[STORAGE-SYSTEM]   Fixing storage permissions recursively..."
        chown -R ${MINIO_UID}:${MINIO_GID} "$DATA_DIR" 2>/dev/null || true
        echo "[STORAGE-SYSTEM]   ✓ Permissions fixed (owner: ${MINIO_UID}:${MINIO_GID})"
    else
        echo "[STORAGE-SYSTEM]   ✓ Permissions already correct"
    fi
    
    chmod 755 "$DATA_DIR" 2>/dev/null || true
else
    echo "[STORAGE-SYSTEM] ⚠️  WARNING: Not running as root"
fi

# Verify directory is writable (test as palmr with detected UID:GID)
su-exec ${MINIO_UID}:${MINIO_GID} sh -c "
    if ! touch '$DATA_DIR/.test-write' 2>/dev/null; then
        echo '[STORAGE-SYSTEM] ❌ FATAL: Still cannot write to $DATA_DIR'
        ls -la '$DATA_DIR'
        echo '[STORAGE-SYSTEM] This should not happen after chown!'
        exit 1
    fi
    rm -f '$DATA_DIR/.test-write'
    echo '[STORAGE-SYSTEM]   ✓ Write test passed'
"

# Generate or reuse password (as root, then chown to palmr)
if [ -f "$PASSWORD_FILE" ]; then
    MINIO_ROOT_PASSWORD=$(cat "$PASSWORD_FILE")
    echo "[STORAGE-SYSTEM] Using existing root password"
else
    MINIO_ROOT_PASSWORD="$(openssl rand -hex 16)"
    echo "$MINIO_ROOT_PASSWORD" > "$PASSWORD_FILE"
    chmod 600 "$PASSWORD_FILE"
    chown ${MINIO_UID}:${MINIO_GID} "$PASSWORD_FILE" 2>/dev/null || true
    echo "[STORAGE-SYSTEM] Generated new root password"
fi

# Export for storage system
export MINIO_ROOT_USER="palmr-minio-admin"
export MINIO_ROOT_PASSWORD="$MINIO_ROOT_PASSWORD"

echo "[STORAGE-SYSTEM] Starting storage server on 0.0.0.0:9379 as user palmr..."

# Execute storage system as palmr user (dropping from root)
exec su-exec ${MINIO_UID}:${MINIO_GID} /usr/local/bin/minio server "$DATA_DIR" \
    --address 0.0.0.0:9379 \
    --console-address 0.0.0.0:9378

