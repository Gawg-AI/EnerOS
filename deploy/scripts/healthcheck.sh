#!/usr/bin/env bash
# EnerOS — Health check script
# Exits 0 if healthy, 1 otherwise
set -euo pipefail

HOST="${ENEROS_HOST:-localhost}"
PORT="${ENEROS_PORT:-8080}"
ENDPOINT="http://${HOST}:${PORT}/health"

response=$(curl -sf --max-time 5 "$ENDPOINT" 2>/dev/null) || {
    echo "FAIL: Cannot reach $ENDPOINT"
    exit 1
}

# Check if response contains "status" field
echo "$response" | grep -q '"status"' || {
    echo "FAIL: Invalid response from $ENDPOINT"
    echo "$response"
    exit 1
}

echo "OK: $ENDPOINT"
echo "$response"
exit 0
