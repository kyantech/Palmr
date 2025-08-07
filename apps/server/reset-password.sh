#!/bin/sh

# Palmr Password Reset Script
# This script allows resetting user passwords from within the Docker container

echo "🔐 Palmr Password Reset Tool"
echo "============================="

# Check if we're in the right directory and set DATABASE_URL
if [ ! -f "package.json" ]; then
    echo "❌ Error: This script must be run from the server directory (/app/server)"
    echo "   Current directory: $(pwd)"
    echo "   Expected: /app/server"
    exit 1
fi

# Set DATABASE_URL if not already set
if [ -z "$DATABASE_URL" ]; then
    export DATABASE_URL="file:/app/server/prisma/palmr.db"
fi

# Ensure database directory exists
mkdir -p /app/server/prisma

# Function to check if tsx is available
check_tsx() {
    # Check if tsx binary exists in node_modules
    if [ -f "node_modules/.bin/tsx" ]; then
        return 0
    fi

    # Fallback: try npx
    if npx tsx --version >/dev/null 2>&1; then
        return 0
    fi

    return 1
}

# Function to install only tsx if missing
install_tsx_only() {
    echo "📦 Installing tsx (quick install)..."
    if command -v pnpm >/dev/null 2>&1; then
        pnpm add tsx --save-dev --silent 2>/dev/null
    elif command -v npm >/dev/null 2>&1; then
        npm install tsx --save-dev --silent 2>/dev/null
    else
        return 1
    fi

    return $?
}

# Function to install all dependencies as fallback
install_all_deps() {
    echo "📦 Installing all dependencies (this may take a moment)..."
    if command -v pnpm >/dev/null 2>&1; then
        pnpm install --silent 2>/dev/null
    elif command -v npm >/dev/null 2>&1; then
        npm install --silent 2>/dev/null
    else
        echo "❌ Error: No package manager found (pnpm/npm)"
        exit 1
    fi
}

# Function to ensure Prisma client is available
ensure_prisma() {
    # Check if Prisma client exists and is valid
    if [ -d "node_modules/@prisma/client" ] && [ -f "node_modules/@prisma/client/index.js" ]; then
        return 0
    fi

    echo "📦 Generating Prisma client..."
    if npx prisma generate --silent >/dev/null 2>&1; then
        echo "✅ Prisma client ready"
        return 0
    else
        echo "❌ Error: Failed to generate Prisma client"
        exit 1
    fi
}

# Quick checks first
echo "🔍 Checking dependencies..."

# Check tsx availability
if check_tsx; then
    echo "✅ tsx is ready"
else
    echo "📦 tsx not found, installing..."

    # Try quick tsx-only install first
    if install_tsx_only && check_tsx; then
        echo "✅ tsx installed successfully"
    else
        echo "⚠️  Quick install failed, installing all dependencies..."
        install_all_deps

        # Final check
        if ! check_tsx; then
            echo "❌ Error: tsx is still not available after full installation"
            echo "   Please check your package.json and node_modules"
            exit 1
        fi
        echo "✅ tsx is now ready"
    fi
fi

# Ensure Prisma client
ensure_prisma

# Check if the TypeScript script exists
if [ ! -f "src/scripts/reset-password.ts" ]; then
    echo "❌ Error: Reset password script not found at src/scripts/reset-password.ts"
    echo "   Available files in src/scripts/:"
    ls -la src/scripts/ 2>/dev/null || echo "   Directory src/scripts/ does not exist"
    exit 1
fi

# All checks passed, run the script
echo "🚀 Starting password reset tool..."
echo ""

# Execute the script using the most reliable method
if [ -f "node_modules/.bin/tsx" ]; then
    node_modules/.bin/tsx src/scripts/reset-password.ts "$@"
else
    npx tsx src/scripts/reset-password.ts "$@"
fi