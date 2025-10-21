# File Expiration Feature - Migration Guide

This guide helps you migrate to the new file expiration feature introduced in Palmr v3.2.5-beta.

## What's New

The file expiration feature allows files to have an optional expiration date. When files expire, they can be automatically deleted by a maintenance script, helping with:

- **Security**: Reducing risk of confidential data exposure
- **Storage Management**: Automatically freeing up server space
- **Convenience**: Eliminating the need for manual file deletion
- **Legal Compliance**: Facilitating adherence to data retention regulations (e.g., GDPR)

## Database Changes

A new optional `expiration` field has been added to the `File` model:

```prisma
model File {
  // ... existing fields
  expiration  DateTime?  // NEW: Optional expiration date
  // ... existing fields
}
```

## Migration Steps

### 1. Backup Your Database

Before running the migration, **always backup your database**:

```bash
# For SQLite (default)
cp apps/server/prisma/palmr.db apps/server/prisma/palmr.db.backup

# Or use the built-in backup command if available
pnpm db:backup
```

### 2. Run the Migration

The migration will automatically run when you start the server, or you can run it manually:

```bash
cd apps/server
pnpm prisma migrate deploy
```

This adds the `expiration` column to the `files` table. **All existing files will have `null` expiration (never expire).**

### 3. Verify the Migration

Check that the migration was successful:

```bash
cd apps/server
pnpm prisma studio
```

Look at the `files` table and verify the new `expiration` column exists.

## API Changes

### File Registration (Upload)

**Before:**
```json
{
  "name": "document.pdf",
  "description": "My document",
  "extension": "pdf",
  "size": 1024000,
  "objectName": "user123/document.pdf"
}
```

**After (optional expiration):**
```json
{
  "name": "document.pdf",
  "description": "My document",
  "extension": "pdf",
  "size": 1024000,
  "objectName": "user123/document.pdf",
  "expiration": "2025-12-31T23:59:59.000Z"
}
```

The `expiration` field is **optional** - omitting it or setting it to `null` means the file never expires.

### File Update

You can now update a file's expiration date:

```bash
PATCH /files/:id
Content-Type: application/json

{
  "expiration": "2026-01-31T23:59:59.000Z"
}
```

To remove expiration:
```json
{
  "expiration": null
}
```

### File Listing

File list responses now include the `expiration` field:

```json
{
  "files": [
    {
      "id": "file123",
      "name": "document.pdf",
      // ... other fields
      "expiration": "2025-12-31T23:59:59.000Z",
      "createdAt": "2025-10-21T10:00:00.000Z",
      "updatedAt": "2025-10-21T10:00:00.000Z"
    }
  ]
}
```

## Setting Up Automatic Cleanup

The file expiration feature includes a maintenance script that automatically deletes expired files.

### Manual Execution

**Dry-run mode** (preview what would be deleted):
```bash
cd apps/server
pnpm cleanup:expired-files
```

**Confirm mode** (actually delete):
```bash
cd apps/server
pnpm cleanup:expired-files:confirm
```

### Automated Scheduling

#### Option 1: Cron Job (Recommended for Linux/Unix)

Add to crontab to run daily at 2 AM:

```bash
crontab -e
```

Add this line:
```
0 2 * * * cd /path/to/Palmr/apps/server && /usr/bin/pnpm cleanup:expired-files:confirm >> /var/log/palmr-cleanup.log 2>&1
```

#### Option 2: Systemd Timer (Linux)

Create `/etc/systemd/system/palmr-cleanup.service`:
```ini
[Unit]
Description=Palmr Expired Files Cleanup
After=network.target

[Service]
Type=oneshot
User=palmr
WorkingDirectory=/path/to/Palmr/apps/server
ExecStart=/usr/bin/pnpm cleanup:expired-files:confirm
StandardOutput=journal
StandardError=journal
```

Create `/etc/systemd/system/palmr-cleanup.timer`:
```ini
[Unit]
Description=Daily Palmr Cleanup
Requires=palmr-cleanup.service

[Timer]
OnCalendar=daily
OnCalendar=02:00
Persistent=true

[Install]
WantedBy=timers.target
```

Enable:
```bash
sudo systemctl enable palmr-cleanup.timer
sudo systemctl start palmr-cleanup.timer
```

#### Option 3: Docker Compose

Add a scheduled service to your `docker-compose.yml`:

```yaml
services:
  palmr-cleanup:
    image: palmr:latest
    command: sh -c "while true; do sleep 86400; pnpm cleanup:expired-files:confirm; done"
    environment:
      - DATABASE_URL=file:/data/palmr.db
    volumes:
      - ./data:/data
      - ./uploads:/uploads
    restart: unless-stopped
```

Or use an external scheduler with a one-shot container:
```yaml
services:
  palmr-cleanup:
    image: palmr:latest
    command: pnpm cleanup:expired-files:confirm
    environment:
      - DATABASE_URL=file:/data/palmr.db
    volumes:
      - ./data:/data
      - ./uploads:/uploads
    restart: "no"
```

## Backward Compatibility

This feature is **fully backward compatible**:

- Existing files automatically have `expiration = null` (never expire)
- The `expiration` field is optional in all API endpoints
- No changes required to existing client code
- Files without expiration dates continue to work exactly as before

## Client Implementation Examples

### JavaScript/TypeScript

```typescript
// Upload file with expiration
const uploadWithExpiration = async (file: File) => {
  // Set expiration to 30 days from now
  const expiration = new Date();
  expiration.setDate(expiration.getDate() + 30);

  const response = await fetch('/api/files', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      name: file.name,
      extension: file.name.split('.').pop(),
      size: file.size,
      objectName: `user/${Date.now()}-${file.name}`,
      expiration: expiration.toISOString(),
    }),
  });

  return response.json();
};

// Update file expiration
const updateExpiration = async (fileId: string, days: number) => {
  const expiration = new Date();
  expiration.setDate(expiration.getDate() + days);

  const response = await fetch(`/api/files/${fileId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      expiration: expiration.toISOString(),
    }),
  });

  return response.json();
};

// Remove expiration (make file permanent)
const removExpiration = async (fileId: string) => {
  const response = await fetch(`/api/files/${fileId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      expiration: null,
    }),
  });

  return response.json();
};
```

### Python

```python
from datetime import datetime, timedelta
import requests

# Upload file with expiration
def upload_with_expiration(file_data):
    expiration = datetime.utcnow() + timedelta(days=30)
    
    response = requests.post('http://localhost:3333/files', json={
        'name': file_data['name'],
        'extension': file_data['extension'],
        'size': file_data['size'],
        'objectName': file_data['objectName'],
        'expiration': expiration.isoformat() + 'Z'
    })
    
    return response.json()

# Update expiration
def update_expiration(file_id, days):
    expiration = datetime.utcnow() + timedelta(days=days)
    
    response = requests.patch(f'http://localhost:3333/files/{file_id}', json={
        'expiration': expiration.isoformat() + 'Z'
    })
    
    return response.json()
```

## Best Practices

1. **Start with dry-run**: Always test the cleanup script in dry-run mode first
2. **Monitor logs**: Keep track of what files are being deleted
3. **User notifications**: Consider notifying users before their files expire
4. **Grace period**: Set expiration dates with a buffer for important files
5. **Backup strategy**: Maintain backups before enabling automatic deletion
6. **Documentation**: Document your expiration policies for users

## Troubleshooting

### Migration Fails

If the migration fails:

1. Check database connectivity
2. Ensure you have write permissions
3. Verify the database file isn't locked
4. Try running `pnpm prisma migrate reset` (WARNING: this will delete all data)

### Cleanup Script Not Deleting Files

1. Verify files have expiration dates set and are in the past
2. Check script is running with `--confirm` flag
3. Review logs for specific errors
3. Ensure script has permissions to delete from storage

### Need to Rollback

If you need to rollback the migration:

```bash
cd apps/server

# View migration history
pnpm prisma migrate status

# Rollback (requires manual SQL for production)
# SQLite example:
sqlite3 prisma/palmr.db "ALTER TABLE files DROP COLUMN expiration;"
```

Note: Prisma doesn't support automatic rollback. You must manually reverse the migration or restore from backup.

## Support

For issues or questions:

- Create an issue on GitHub
- Check the documentation at https://palmr.kyantech.com.br
- Review the scripts README at `apps/server/src/scripts/README.md`

## Changelog

### Version 3.2.5-beta

- Added optional `expiration` field to File model
- Created `cleanup-expired-files` maintenance script
- Updated File DTOs to support expiration in create/update operations
- Added API documentation for expiration field
- Created comprehensive documentation for setup and usage
