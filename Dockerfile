FROM node:24-alpine AS base

# Install system dependencies
RUN apk add --no-cache \
  gcompat \
  supervisor \
  curl \
  wget \
  openssl \
  su-exec

# Enable pnpm
RUN corepack enable pnpm

# Install storage system for S3-compatible storage
COPY infra/install-minio.sh /tmp/install-minio.sh
RUN chmod +x /tmp/install-minio.sh && /tmp/install-minio.sh

# Install storage client (mc) for appropriate architecture
COPY infra/install-mc.sh /tmp/install-mc.sh
RUN chmod +x /tmp/install-mc.sh && /tmp/install-mc.sh

# Set working directory
WORKDIR /app

# === SERVER BUILD STAGE ===
FROM base AS server-deps
WORKDIR /app/server

# Copy server package files
COPY apps/server/package*.json ./
COPY apps/server/pnpm-lock.yaml ./

# Install server dependencies
RUN pnpm install --frozen-lockfile

FROM base AS server-builder
WORKDIR /app/server

# Copy server dependencies
COPY --from=server-deps /app/server/node_modules ./node_modules

# Copy server source code
COPY apps/server/ ./

# Generate Prisma client
RUN npx prisma generate

# Build server
RUN pnpm build

# === WEB BUILD STAGE ===
FROM base AS web-deps
WORKDIR /app/web

# Copy web package files
COPY apps/web/package.json apps/web/pnpm-lock.yaml ./

# Install web dependencies
RUN pnpm install --frozen-lockfile

FROM base AS web-builder
WORKDIR /app/web

# Copy web dependencies
COPY --from=web-deps /app/web/node_modules ./node_modules

# Copy web source code
COPY apps/web/ ./

# Set environment variables for build
ENV NEXT_TELEMETRY_DISABLED=1
ENV NODE_ENV=production

# Build web application
RUN pnpm run build

# === PRODUCTION STAGE ===
FROM base AS runner

# Set production environment
ENV NODE_ENV=production
ENV NEXT_TELEMETRY_DISABLED=1
ENV API_BASE_URL=http://127.0.0.1:3333

# Define build arguments for user/group configuration (defaults to current values)
ARG PALMR_UID=1001
ARG PALMR_GID=1001

# Create application user with configurable UID/GID
RUN addgroup --system --gid ${PALMR_GID} nodejs
RUN adduser --system --uid ${PALMR_UID} --ingroup nodejs palmr

# Create application directories 
RUN mkdir -p /app/palmr-app /app/web /app/infra /home/palmr/.npm /home/palmr/.cache
RUN chown -R palmr:nodejs /app /home/palmr

# === Copy Server Files to /app/palmr-app (separate from /app/server for bind mounts) ===
WORKDIR /app/palmr-app

# Copy server production files
COPY --from=server-builder --chown=palmr:nodejs /app/server/dist ./dist
COPY --from=server-builder --chown=palmr:nodejs /app/server/node_modules ./node_modules
COPY --from=server-builder --chown=palmr:nodejs /app/server/prisma ./prisma
COPY --from=server-builder --chown=palmr:nodejs /app/server/package.json ./

# Copy password reset script and make it executable
COPY --from=server-builder --chown=palmr:nodejs /app/server/reset-password.sh ./
COPY --from=server-builder --chown=palmr:nodejs /app/server/src/scripts/ ./src/scripts/
RUN chmod +x ./reset-password.sh

# Copy seed file to the shared location for bind mounts
RUN mkdir -p /app/server/prisma
COPY --from=server-builder --chown=palmr:nodejs /app/server/prisma/seed.js /app/server/prisma/seed.js

# === Copy Web Files ===
WORKDIR /app/web

# Copy web production files
COPY --from=web-builder --chown=palmr:nodejs /app/web/public ./public
COPY --from=web-builder --chown=palmr:nodejs /app/web/.next/standalone ./
COPY --from=web-builder --chown=palmr:nodejs /app/web/.next/static ./.next/static

# === Setup Supervisor ===
WORKDIR /app

# Create supervisor configuration
RUN mkdir -p /etc/supervisor/conf.d

# Copy server start script and configuration files
COPY infra/server-start.sh /app/server-start.sh
COPY infra/start-minio.sh /app/start-minio.sh
COPY infra/minio-setup.sh /app/minio-setup.sh
COPY infra/load-minio-credentials.sh /app/load-minio-credentials.sh
COPY infra/configs.json /app/infra/configs.json
COPY infra/providers.json /app/infra/providers.json
COPY infra/check-missing.js /app/infra/check-missing.js
RUN chmod +x /app/server-start.sh /app/start-minio.sh /app/minio-setup.sh /app/load-minio-credentials.sh
RUN chown -R palmr:nodejs /app/server-start.sh /app/start-minio.sh /app/minio-setup.sh /app/load-minio-credentials.sh /app/infra

# Copy supervisor configuration
COPY infra/supervisord.conf /etc/supervisor/conf.d/supervisord.conf

# Create main startup script
COPY <<EOF /app/start.sh
#!/bin/sh
set -e

echo "Starting Palmr Application..."
echo "Storage Mode: \${ENABLE_S3:-false}"
echo "Secure Site: \${SECURE_SITE:-false}"
echo "Encryption: \${DISABLE_FILESYSTEM_ENCRYPTION:-true}"
echo "Database: SQLite"

# Set global environment variables
export DATABASE_URL="file:/app/server/prisma/palmr.db"
export NEXT_PUBLIC_DEFAULT_LANGUAGE=\${DEFAULT_LANGUAGE:-en-US}

# Ensure /app/server directory exists for bind mounts
mkdir -p /app/server/uploads /app/server/temp-uploads /app/server/prisma /app/server/minio-data

# CRITICAL: Fix permissions BEFORE starting any services
# This runs on EVERY startup to handle updates and corrupted metadata
echo "🔐 Fixing permissions for internal storage..."

# DYNAMIC: Detect palmr user's actual UID and GID
# Works with any Docker --user configuration
PALMR_UID=\$(id -u palmr 2>/dev/null || echo "1001")
PALMR_GID=\$(id -g palmr 2>/dev/null || echo "1001")
echo "   Target user: palmr (UID:\$PALMR_UID, GID:\$PALMR_GID)"

# ALWAYS remove storage system metadata to prevent corruption issues
# This is safe - storage system recreates it automatically
# User data (files) are NOT in .minio.sys, they're safe
if [ -d "/app/server/minio-data/.minio.sys" ]; then
    echo "   🧹 Cleaning storage system metadata (safe, auto-regenerated)..."
    rm -rf /app/server/minio-data/.minio.sys 2>/dev/null || true
fi

# Fix ownership and permissions (safe for updates)
echo "   🔧 Setting ownership and permissions..."
chown -R \$PALMR_UID:\$PALMR_GID /app/server 2>/dev/null || echo "   ⚠️  chown skipped"
chmod -R 755 /app/server 2>/dev/null || echo "   ⚠️  chmod skipped"

# Verify critical directories are writable
if touch /app/server/.test-write 2>/dev/null; then
    rm -f /app/server/.test-write
    echo "   ✅ Storage directory is writable"
else
    echo "   ❌ FATAL: /app/server is NOT writable!"
    echo "   Check Docker volume permissions"
    ls -la /app/server 2>/dev/null || true
fi

echo "✅ Storage ready, starting services..."

# Start supervisor
exec /usr/bin/supervisord -c /etc/supervisor/conf.d/supervisord.conf
EOF

RUN chmod +x /app/start.sh

# Create volume mount points for bind mounts
VOLUME ["/app/server"]

# Expose ports
EXPOSE 3333 5487 9379 9378

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
  CMD curl -f http://localhost:5487 || exit 1

# Start application
CMD ["/app/start.sh"]