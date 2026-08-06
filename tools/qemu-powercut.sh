#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 4 ]; then
    echo "usage: qemu-powercut.sh <log> <exact-marker> <qemu> <args...>" >&2
    exit 2
fi

log="$1"
marker="$2"
shift 2
: >"${log}"

fifo_dir="$(mktemp -d /tmp/eneros-qemu-powercut.XXXXXX)"
fifo="${fifo_dir}/serial"
mkfifo "${fifo}"
exec 3<>"${fifo}"

"$@" >"${fifo}" 2>&1 &
qemu_pid=$!
if [ -z "${qemu_pid}" ]; then
    echo "power-cut controller did not obtain a QEMU pid" >&2
    exit 40
fi

for ((poll = 0; poll < 600; poll++)); do
    line=""
    if IFS= read -r -t 0.1 line <&3; then
        printf '%s\n' "${line}" >>"${log}"
    fi
    if [[ "${line}" == *"${marker}"* ]]; then
        kill -STOP "${qemu_pid}"
        kill -KILL "${qemu_pid}"
        set +e
        wait "${qemu_pid}"
        status=$?
        set -e
        if [ "${status}" -ne 137 ]; then
            echo "QEMU power removal returned ${status}, expected 137" >&2
            rm -f "${fifo}"
            rmdir "${fifo_dir}"
            exit 43
        fi
        rm -f "${fifo}"
        rmdir "${fifo_dir}"
        exit 0
    fi
    if ! kill -0 "${qemu_pid}" 2>/dev/null; then
        set +e
        wait "${qemu_pid}"
        status=$?
        set -e
        echo "QEMU exited ${status} before marker ${marker}" >&2
        rm -f "${fifo}"
        rmdir "${fifo_dir}"
        exit 41
    fi
    sleep 0.01
done

kill -KILL "${qemu_pid}"
set +e
wait "${qemu_pid}"
set -e
rm -f "${fifo}"
rmdir "${fifo_dir}"
echo "QEMU did not reach marker ${marker} within the bounded poll budget" >&2
exit 42
