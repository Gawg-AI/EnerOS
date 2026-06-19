#!/bin/bash
# EnerOS boot integration test
# Tests that the OS image boots correctly in QEMU and the application layer starts
set -euo pipefail

# Configuration
ARCH="${ARCH:-x86_64}"
IMAGE="${IMAGE:-../image-builder/output/eneros-$ARCH.img}"
QEMU_MEMORY="${QEMU_MEMORY:-2G}"
QEMU_CPUS="${QEMU_CPUS:-4}"
TIMEOUT="${TIMEOUT:-120}"  # 2 minutes timeout
HEALTH_CHECK_URL="http://localhost:8080/health"
HEALTH_CHECK_PORT="8080"

echo "=== EnerOS Boot Integration Test ==="
echo "Image: $IMAGE"
echo "Architecture: $ARCH"
echo "QEMU Memory: $QEMU_MEMORY"
echo "Timeout: ${TIMEOUT}s"

# Check if image exists
if [ ! -f "$IMAGE" ]; then
    echo "FAIL: Image not found: $IMAGE"
    echo "Please run os/image-builder/build.sh first"
    exit 1
fi

# Check if QEMU is available
QEMU_BIN=""
case "$ARCH" in
    x86_64)  QEMU_BIN="qemu-system-x86_64" ;;
    aarch64) QEMU_BIN="qemu-system-aarch64" ;;
    *) echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

if ! command -v "$QEMU_BIN" > /dev/null 2>&1; then
    echo "FAIL: $QEMU_BIN not found"
    echo "Please install QEMU: apt install qemu-system"
    exit 1
fi

# Create temporary log file
LOG_FILE=$(mktemp /tmp/eneros-boot-test-XXXXXX.log)
trap "rm -f $LOG_FILE" EXIT

echo ">>> Starting QEMU with EnerOS image..."

# Start QEMU in background
# - Serial console output to log file
# - Port forwarding for health check
# - No graphics (headless)
qemu_cmd=(
    "$QEMU_BIN"
    -drive "file=$IMAGE,format=raw"
    -m "$QEMU_MEMORY"
    -smp "$QEMU_CPUS"
    -nographic
    -serial "file:$LOG_FILE"
    -netdev "user,id=net0,hostfwd=tcp::$HEALTH_CHECK_PORT-:8080"
    -device "virtio-net-pci,netdev=net0"
)

if [ "$ARCH" = "x86_64" ] && [ -e /dev/kvm ]; then
    qemu_cmd+=(--enable-kvm)
fi

# Start QEMU
"${qemu_cmd[@]}" &
QEMU_PID=$!

echo "QEMU PID: $QEMU_PID"

# Wait for boot and health check
echo ">>> Waiting for EnerOS to boot..."
START_TIME=$(date +%s)

boot_success=false
health_check_success=false

while true; do
    CURRENT_TIME=$(date +%s)
    ELAPSED=$((CURRENT_TIME - START_TIME))

    if [ $ELAPSED -ge $TIMEOUT ]; then
        echo "FAIL: Timeout after ${TIMEOUT}s"
        break
    fi

    # Check if QEMU is still running
    if ! kill -0 $QEMU_PID 2>/dev/null; then
        echo "FAIL: QEMU process exited unexpectedly"
        break
    fi

    # Check boot log for success indicators
    if [ -f "$LOG_FILE" ]; then
        if grep -q "EnerOS init starting" "$LOG_FILE" && [ "$boot_success" = false ]; then
            echo "  [OK] eneros-init started"
            boot_success=true
        fi

        if grep -q "EnerOS init initialization complete" "$LOG_FILE" && [ "$boot_success" = true ]; then
            echo "  [OK] eneros-init initialization complete"
        fi

        if grep -q "Service startup order" "$LOG_FILE"; then
            echo "  [OK] Service startup order determined"
        fi
    fi

    # Try health check
    if [ "$boot_success" = true ]; then
        if curl -sf "$HEALTH_CHECK_URL" > /dev/null 2>&1; then
            echo "  [OK] Health check passed"
            health_check_success=true
            break
        fi
    fi

    sleep 2
done

# Print boot log on failure
if [ "$health_check_success" = false ]; then
    echo ""
    echo "=== Boot Log (last 50 lines) ==="
    tail -50 "$LOG_FILE" 2>/dev/null || echo "(no log file)"
fi

# Cleanup
kill $QEMU_PID 2>/dev/null || true
wait $QEMU_PID 2>/dev/null || true

# Result
echo ""
if [ "$health_check_success" = true ]; then
    echo "=== PASS: EnerOS boot test successful ==="
    exit 0
else
    echo "=== FAIL: EnerOS boot test failed ==="
    exit 1
fi
