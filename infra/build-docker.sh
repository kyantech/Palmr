#!/bin/bash

# Use argument if provided, otherwise ask interactively
if [ -n "$1" ]; then
    TAG="$1"
    echo "🏷️  Using provided tag: $TAG"
else
    echo "🏷️  Please enter a tag for the build (e.g., v1.0.0, production, beta):"
    read -p "Tag: " TAG
    
    # Check if tag was provided
    if [ -z "$TAG" ]; then
        echo "❌ Error: Tag cannot be empty"
        echo "Please run the script again and provide a valid tag"
        exit 1
    fi
fi

echo "🚀 Building Palmr Unified Image for AMD64 and ARM..."
echo "📦 Building tags: latest and $TAG"

# Ensure buildx is available and create/use a builder instance
docker buildx create --name palmr-builder --use 2>/dev/null || docker buildx use palmr-builder

# Detect local platform for loading
LOCAL_PLATFORM=$(docker version --format '{{.Server.Arch}}')
if [ "$LOCAL_PLATFORM" = "aarch64" ] || [ "$LOCAL_PLATFORM" = "arm64" ]; then
    LOAD_PLATFORM="linux/arm64"
else
    LOAD_PLATFORM="linux/amd64"
fi

echo "Detected local platform: $LOCAL_PLATFORM, will load: $LOAD_PLATFORM"

# Build for local platform and load
docker buildx build \
    --platform $LOAD_PLATFORM \
    --no-cache \
    -t kyantech/palmr:latest \
    -t kyantech/palmr:$TAG \
    --load \
    .

if [ $? -eq 0 ]; then
    echo "✅ Multi-platform build completed successfully!"
    echo ""
    echo "Built for platforms: linux/amd64, linux/arm64"
    echo "Built tags: palmr:latest and palmr:$TAG"
    echo ""
    echo "Access points:"
    echo "- API: http://localhost:3333"
    echo "- Web App: http://localhost:5487"
    echo ""
    echo "Read the docs for more information"
else
    echo "❌ Build failed!"
    exit 1
fi 