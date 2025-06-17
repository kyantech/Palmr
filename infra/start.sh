#!/bin/sh
set -e

echo "Starting Palmr Application..."
echo "Storage Mode: ${ENABLE_S3:-false}"
echo "Database: SQLite"

# Runtime UID/GID configuration - only apply if environment variables are set
if [ -n "${PALMR_UID}" ] || [ -n "${PALMR_GID}" ]; then
    RUNTIME_UID=${PALMR_UID:-${PALMR_UID}}
    RUNTIME_GID=${PALMR_GID:-${PALMR_GID}}
    
    echo "Runtime UID/GID configuration detected: UID=$RUNTIME_UID, GID=$RUNTIME_GID"
    
    # Get current user/group IDs
    CURRENT_UID=$(id -u palmr 2>/dev/null || echo "${PALMR_UID}")
    CURRENT_GID=$(id -g palmr 2>/dev/null || echo "${PALMR_GID}")
    
    # Only modify if different from current
    if [ "$CURRENT_UID" != "$RUNTIME_UID" ] || [ "$CURRENT_GID" != "$RUNTIME_GID" ]; then
        echo "Adjusting user/group IDs from $CURRENT_UID:$CURRENT_GID to $RUNTIME_UID:$RUNTIME_GID"
        
        # Modify group if needed
        if [ "$CURRENT_GID" != "$RUNTIME_GID" ]; then
            if getent group $RUNTIME_GID >/dev/null 2>&1; then
                EXISTING_GROUP=$(getent group $RUNTIME_GID | cut -d: -f1)
                echo "Using existing group with GID $RUNTIME_GID: $EXISTING_GROUP"
                usermod -g $EXISTING_GROUP palmr 2>/dev/null || echo "Warning: Could not change user group"
            else
                groupmod -g $RUNTIME_GID nodejs 2>/dev/null || echo "Warning: Could not modify group GID"
            fi
        fi
        
        # Modify user if needed
        if [ "$CURRENT_UID" != "$RUNTIME_UID" ]; then
            if getent passwd $RUNTIME_UID >/dev/null 2>&1; then
                EXISTING_USER=$(getent passwd $RUNTIME_UID | cut -d: -f1)
                echo "Warning: UID $RUNTIME_UID already exists as user '$EXISTING_USER'"
                echo "Container will continue but may have permission issues"
            else
                usermod -u $RUNTIME_UID palmr 2>/dev/null || echo "Warning: Could not modify user UID"
            fi
        fi
        
        # Update file ownership for application directories
        echo "Updating file ownership for application directories..."
        chown -R palmr:nodejs /app /home/palmr 2>/dev/null || echo "Warning: Could not update all file ownership"
    else
        echo "Runtime UID/GID matches current values, no changes needed"
    fi
else
    echo "No runtime UID/GID configuration provided, using defaults"
fi

# Ensure storage directories exist with correct permissions
mkdir -p /app/server/uploads /app/server/temp-chunks /app/server/uploads/logo /app/server/prisma /app/server/db
chown -R palmr:nodejs /app/server/uploads /app/server/temp-chunks /app/server/prisma /app/server/db 2>/dev/null || echo "Warning: Could not set permissions on storage directories"

# Start supervisor
exec /usr/bin/supervisord -c /etc/supervisor/conf.d/supervisord.conf