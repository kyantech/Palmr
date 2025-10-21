# Palmr Maintenance Scripts

This directory contains maintenance scripts for the Palmr server application.

## Available Scripts

### 1. Cleanup Expired Files (`cleanup-expired-files.ts`)

Automatically deletes files that have reached their expiration date. This script is designed to be run periodically (e.g., via cron job) to maintain storage hygiene and comply with data retention policies.

#### Features

- **Automatic Deletion**: Removes both the file metadata from the database and the actual file from storage
- **Dry-Run Mode**: Preview what would be deleted without actually removing files
- **Storage Agnostic**: Works with both filesystem and S3-compatible storage
- **Detailed Logging**: Provides clear output about what files were found and deleted
- **Error Handling**: Continues processing even if individual files fail to delete

#### Usage

**Dry-run mode** (preview without deleting):
```bash
pnpm cleanup:expired-files
```

**Confirm mode** (actually delete expired files):
```bash
pnpm cleanup:expired-files:confirm
```

Or directly with tsx:
```bash
tsx src/scripts/cleanup-expired-files.ts --confirm
```

#### Output Example

```
🧹 Starting expired files cleanup...
📦 Storage mode: Filesystem
📊 Found 2 expired files

🗑️  Expired files to be deleted:
  - document.pdf (2.45 MB) - Expired: 2025-10-20T10:30:00.000Z
  - image.jpg (1.23 MB) - Expired: 2025-10-21T08:15:00.000Z

🗑️  Deleting expired files...
  ✓ Deleted: document.pdf
  ✓ Deleted: image.jpg

✅ Cleanup complete!
   Deleted: 2 files (3.68 MB)
```

#### Setting Up Automated Cleanup

To run this script automatically, you can set up a cron job:

##### Using crontab (Linux/Unix)

1. Edit your crontab:
   ```bash
   crontab -e
   ```

2. Add a line to run the cleanup daily at 2 AM:
   ```
   0 2 * * * cd /path/to/Palmr/apps/server && pnpm cleanup:expired-files:confirm >> /var/log/palmr-cleanup.log 2>&1
   ```

##### Using systemd timer (Linux)

1. Create a service file `/etc/systemd/system/palmr-cleanup-expired.service`:
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

2. Create a timer file `/etc/systemd/system/palmr-cleanup-expired.timer`:
   ```ini
   [Unit]
   Description=Run Palmr Expired Files Cleanup Daily
   Requires=palmr-cleanup-expired.service

   [Timer]
   OnCalendar=daily
   OnCalendar=02:00
   Persistent=true

   [Install]
   WantedBy=timers.target
   ```

3. Enable and start the timer:
   ```bash
   sudo systemctl enable palmr-cleanup-expired.timer
   sudo systemctl start palmr-cleanup-expired.timer
   ```

##### Using Docker

If running Palmr in Docker, you can add the cleanup command to your compose file or create a separate service:

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

Then schedule it with your host's cron or a container orchestration tool.

#### Best Practices

1. **Test First**: Always run in dry-run mode first to preview what will be deleted
2. **Monitor Logs**: Keep track of cleanup operations by logging output
3. **Regular Schedule**: Run the cleanup at least daily to prevent storage bloat
4. **Off-Peak Hours**: Schedule cleanup during low-traffic periods
5. **Backup Strategy**: Ensure you have backups before enabling automatic deletion

### 2. Cleanup Orphan Files (`cleanup-orphan-files.ts`)

Removes file records from the database that no longer have corresponding files in storage. This can happen if files are manually deleted from storage or if an upload fails partway through.

#### Usage

**Dry-run mode**:
```bash
pnpm cleanup:orphan-files
```

**Confirm mode**:
```bash
pnpm cleanup:orphan-files:confirm
```

## File Expiration Feature

Files in Palmr can now have an optional expiration date. When a file expires, it becomes eligible for automatic deletion by the cleanup script.

### Setting Expiration During Upload

When registering a file, include the `expiration` field with an ISO 8601 datetime string:

```json
{
  "name": "document.pdf",
  "description": "Confidential document",
  "extension": "pdf",
  "size": 2048000,
  "objectName": "user123/document.pdf",
  "expiration": "2025-12-31T23:59:59.000Z"
}
```

### Updating File Expiration

You can update a file's expiration date at any time:

```bash
PATCH /files/:id
Content-Type: application/json

{
  "expiration": "2026-01-31T23:59:59.000Z"
}
```

To remove an expiration date (file never expires):

```json
{
  "expiration": null
}
```

### Use Cases

- **Temporary Shares**: Share files that automatically delete after a certain period
- **Compliance**: Meet data retention requirements (e.g., GDPR)
- **Storage Management**: Automatically free up space by removing old files
- **Security**: Reduce risk of sensitive data exposure by limiting file lifetime
- **Trial Periods**: Automatically clean up files from trial or demo accounts

## Security Considerations

- Scripts run with the same permissions as the application
- Deleted files cannot be recovered unless backups are in place
- Always test scripts in a development environment first
- Monitor script execution and review logs regularly
- Consider implementing file versioning or soft deletes for critical data

## Troubleshooting

### Script Fails to Connect to Database

Ensure the `DATABASE_URL` environment variable is set correctly in your `.env` file.

### Files Not Being Deleted

1. Check that files actually have an expiration date set
2. Verify the expiration date is in the past
3. Ensure the script has appropriate permissions to delete files
4. Check application logs for specific error messages

### Storage Provider Issues

If using S3-compatible storage, ensure:
- Credentials are valid and have delete permissions
- Network connectivity to the S3 endpoint is working
- Bucket exists and is accessible

## Contributing

When adding new maintenance scripts:

1. Follow the existing naming convention
2. Include dry-run and confirm modes
3. Provide clear logging output
4. Handle errors gracefully
5. Update this README with usage instructions
