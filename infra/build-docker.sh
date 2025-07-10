#!/bin/bash

echo "🏷️  Please enter a tag for the build (e.g., v1.0.0, production, beta):"
read -p "Tag: " TAG

if [ -z "$TAG" ]; then
    echo "❌ Error: Tag cannot be empty"
    echo "Please run the script again and provide a valid tag"
    exit 1
fi

echo "🚀 Building Palmr Unified Image for AMD64 and ARM..."
echo "📦 Building tags: latest and $TAG"

docker buildx create --name palmr-builder --use 2>/dev/null || docker buildx use palmr-builder

docker buildx build \
    --platform linux/amd64,linux/arm64 \
    --no-cache \
    -t kyantech/palmr:latest \
    -t kyantech/palmr:$TAG \
    --push \
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