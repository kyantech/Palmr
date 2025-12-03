#!/bin/sh
# Start storage system with persistent root password
# This script MUST run as root to fix permissions, then drops to palmr user

set -e

DATA_DIR="/app/server/minio-data"
PASSWORD_FILE="/app/server/.minio-root-password"
MINIO_USER="palmr"

echo "[STORAGE-SYSTEM] Initializing storage..."

# DYNAMIC: Detect palmr user's actual UID and GID
# This works with any Docker user configuration
MINIO_UID=$(id -u $MINIO_USER 2>/dev/null || echo "1001")
MINIO_GID=$(id -g $MINIO_USER 2>/dev/null || echo "1001")

echo "[STORAGE-SYSTEM]   Target user: $MINIO_USER (UID:$MINIO_UID, GID:$MINIO_GID)"

# CRITICAL: Fix permissions as root (supervisor runs this as root via user=root)
# This MUST happen before dropping to palmr user
if [ "$(id -u)" = "0" ]; then
    echo "[STORAGE-SYSTEM]   Fixing permissions (running as root)..."
    
    # Clean metadata
    if [ -d "$DATA_DIR/.minio.sys" ]; then
        echo "[STORAGE-SYSTEM]   Cleaning metadata..."
        rm -rf "$DATA_DIR/.minio.sys" 2>/dev/null || true
    fi
    
    # Ensure directory exists
    mkdir -p "$DATA_DIR"
    
    # FIX: Change ownership to palmr (using detected UID:GID)
    chown -R ${MINIO_UID}:${MINIO_GID} "$DATA_DIR" 2>/dev/null || {
        echo "[STORAGE-SYSTEM] ⚠️  chown -R failed, trying non-recursive..."
        chown ${MINIO_UID}:${MINIO_GID} "$DATA_DIR" 2>/dev/null || true
    }
    
    chmod 755 "$DATA_DIR" 2>/dev/null || true
    
    # Force filesystem sync to ensure changes are visible immediately
    sync
    
    echo "[STORAGE-SYSTEM]   ✓ Permissions fixed (owner: ${MINIO_UID}:${MINIO_GID})"
else
    echo "[STORAGE-SYSTEM] ⚠️  WARNING: Not running as root, cannot fix permissions"
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

