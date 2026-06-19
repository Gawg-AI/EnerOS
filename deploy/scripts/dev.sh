#!/usr/bin/env bash
# EnerOS — Development start script
# Usage: ./deploy/scripts/dev.sh [--json-log] [--tls-cert <path> --tls-key <path>]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
cd "$PROJECT_ROOT"

# Default config
CONFIG_FILE="${ENEROS_CONFIG:-eneros.toml}"

# Parse args
ARGS=("--config" "$CONFIG_FILE")
while [[ $# -gt 0 ]]; do
    case $1 in
        --json-log)
            ARGS+=("--json-log")
            shift
            ;;
        --tls-cert)
            ARGS+=("--tls-cert" "$2")
            shift 2
            ;;
        --tls-key)
            ARGS+=("--tls-key" "$2")
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [--json-log] [--tls-cert <path> --tls-key <path>]"
            exit 0
            ;;
        *)
            ARGS+=("$1")
            shift
            ;;
    esac
done

echo "Starting EnerOS in development mode..."
echo "Config: $CONFIG_FILE"
echo "Command: cargo run --package eneros-api -- run ${ARGS[*]}"

export CARGO_INCREMENTAL=0
export RUST_LOG="${RUST_LOG:-info}"

exec cargo run --package eneros-api -- run "${ARGS[@]}"
