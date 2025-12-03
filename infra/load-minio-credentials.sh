#!/bin/sh
# Load storage system credentials and export as environment variables

CREDENTIALS_FILE="/app/server/.minio-credentials"

if [ -f "$CREDENTIALS_FILE" ]; then
    echo "[PALMR] Loading storage system credentials..."
    
    # Read and export each line
    while IFS= read -r line; do
        # Skip empty lines and comments
        case "$line" in
            ''|'#'*) continue ;;
        esac
        
        # Export the variable
        export "$line"
    done < "$CREDENTIALS_FILE"
    
    echo "[PALMR] ✓ Storage system credentials loaded"
else
    echo "[PALMR] ⚠ Storage system credentials not found at $CREDENTIALS_FILE"
    echo "[PALMR] ⚠ No S3 configured - check your setup"
fi


