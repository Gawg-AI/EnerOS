#!/bin/bash
# verify-boot-params.sh — Verify RT kernel boot parameters on EnerOS
#
# Usage: sudo ./verify-boot-params.sh
#        (chmod +x verify-boot-params.sh first if not executable)
#
# Checks:
#   1. /proc/cmdline contains the required RT isolation parameters
#   2. /sys/kernel/realtime exists and equals 1 (PREEMPT_RT kernel)
#   3. /sys/devices/system/cpu/isolated reflects CPU isolation
#
# Exit codes: 0 = all checks passed, 1 = one or more checks failed

set -euo pipefail

PASS=0
FAIL=0

CMDLINE=$(cat /proc/cmdline)

check_param() {
    local param="$1"
    if [[ " $CMDLINE " == *" $param "* ]]; then
        echo "PASS: $param found in /proc/cmdline"
        PASS=$((PASS + 1))
    else
        echo "FAIL: $param missing from /proc/cmdline"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== EnerOS RT Boot Parameter Verification ==="
echo ""

# 1. Required RT boot parameters
check_param "isolcpus=2,3"
check_param "nohz_full=2,3"
check_param "rcu_nocbs=2,3"
check_param "irqaffinity=0,1"

# 2. PREEMPT_RT kernel marker
if [[ -f /sys/kernel/realtime ]]; then
    RT_VAL=$(cat /sys/kernel/realtime)
    if [[ "$RT_VAL" == "1" ]]; then
        echo "PASS: PREEMPT_RT active (/sys/kernel/realtime=1)"
        PASS=$((PASS + 1))
    else
        echo "FAIL: /sys/kernel/realtime=$RT_VAL (expected 1)"
        FAIL=$((FAIL + 1))
    fi
else
    echo "FAIL: /sys/kernel/realtime not found (not a PREEMPT_RT kernel)"
    FAIL=$((FAIL + 1))
fi

# 3. CPU isolation verification
if [[ -f /sys/devices/system/cpu/isolated ]]; then
    ISOLATED=$(cat /sys/devices/system/cpu/isolated)
    if [[ "$ISOLATED" == "2,3" ]]; then
        echo "PASS: isolated CPUs = $ISOLATED"
        PASS=$((PASS + 1))
    else
        echo "FAIL: isolated CPUs = '$ISOLATED' (expected 2,3)"
        FAIL=$((FAIL + 1))
    fi
else
    echo "FAIL: /sys/devices/system/cpu/isolated not found"
    FAIL=$((FAIL + 1))
fi

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="

if [[ "$FAIL" -eq 0 ]]; then
    exit 0
else
    exit 1
fi
