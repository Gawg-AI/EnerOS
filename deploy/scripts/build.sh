#!/usr/bin/env bash
# EnerOS — Production build script
# Builds the release binary and Docker image
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
cd "$PROJECT_ROOT"

VERSION="${1:-$(date +%Y%m%d%H%M%S)}"
IMAGE_NAME="${ENEROS_IMAGE:-eneros}"
TAG="${IMAGE_NAME}:${VERSION}"
LATEST_TAG="${IMAGE_NAME}:latest"

echo "=== EnerOS Production Build ==="
echo "Version: $VERSION"
echo "Image:   $TAG"
echo ""

# Step 1: Build release binary
echo "[1/3] Building release binary..."
export CARGO_INCREMENTAL=0
cargo build --release --package eneros-api
echo "  ✓ Binary built: target/release/eneros-api"

# Step 2: Run tests
echo "[2/3] Running tests..."
cargo test --workspace --release -- --test-threads=4
echo "  ✓ All tests passed"

# Step 3: Build Docker image
echo "[3/3] Building Docker image..."
docker build -t "$TAG" -t "$LATEST_TAG" -f deploy/docker/Dockerfile .
echo "  ✓ Docker image built: $TAG"
echo ""
echo "=== Build Complete ==="
echo "To run: docker compose -f deploy/docker/docker-compose.yml up -d"
echo "To push: docker push $TAG"
