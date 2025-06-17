FROM node:20-alpine AS base

# Install system dependencies (removed netcat-openbsd since we no longer need to wait for PostgreSQL)
RUN apk add --no-cache \
  gcompat \
  supervisor \
  curl

# Enable pnpm
RUN corepack enable pnpm

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

# Create application directories and set permissions
# Include storage directories for filesystem mode and SQLite database directory
RUN mkdir -p /app/server /app/web /home/palmr/.npm /home/palmr/.cache \
  /app/server/uploads /app/server/temp-chunks /app/server/uploads/logo \
  /app/server/prisma /app/server/db
RUN chown -R palmr:nodejs /app /home/palmr

# === Copy Server Files ===
WORKDIR /app/server

# Copy server production files
COPY --from=server-builder --chown=palmr:nodejs /app/server/dist ./dist
COPY --from=server-builder --chown=palmr:nodejs /app/server/node_modules ./node_modules
COPY --from=server-builder --chown=palmr:nodejs /app/server/prisma ./prisma
COPY --from=server-builder --chown=palmr:nodejs /app/server/package.json ./

# Copy password reset script and make it executable
COPY --from=server-builder --chown=palmr:nodejs /app/server/reset-password.sh ./
COPY --from=server-builder --chown=palmr:nodejs /app/server/src/scripts/ ./src/scripts/
RUN chmod +x ./reset-password.sh

# Ensure storage directories have correct permissions
RUN chown -R palmr:nodejs /app/server/uploads /app/server/temp-chunks /app/server/prisma /app/server/db

# === Copy Web Files ===
WORKDIR /app/web

# Copy web production files
COPY --from=web-builder --chown=palmr:nodejs /app/web/public ./public
COPY --from=web-builder --chown=palmr:nodejs /app/web/.next/standalone ./
COPY --from=web-builder --chown=palmr:nodejs /app/web/.next/static ./.next/static

# === Setup Supervisor ===
WORKDIR /app

# Create supervisor configuration
RUN mkdir -p /etc/supervisor/conf.d /var/log/supervisor

# Copy server start script
COPY infra/server-start.sh /app/server-start.sh
RUN chmod +x /app/server-start.sh
RUN chown palmr:nodejs /app/server-start.sh

# Copy supervisor configuration
COPY infra/supervisord.conf /etc/supervisor/conf.d/supervisord.conf

# Create main startup script with UID/GID runtime support
COPY infra/start.sh /app/start.sh

RUN chmod +x /app/start.sh

# Create volume mount points for persistent storage (filesystem mode and SQLite database)
# VOLUME ["/app/server/uploads", "/app/server/temp-chunks", "/app/server/prisma"]

# Expose ports
EXPOSE 3333 5487

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
  CMD curl -f http://localhost:5487 || exit 1

# Start application
CMD ["/app/start.sh"] 