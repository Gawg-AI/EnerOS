#!/usr/bin/env python3
"""33 WP-33.5：QEMU socket 串口客户端——读全部串口到日志，
并在 root-task 绑定 pl011 IRQ 后注入 UART RX 字符（触发 RX 中断计数）。"""
import socket, sys, time, threading

SOCK = sys.argv[1] if len(sys.argv) > 1 else "/tmp/wp335-uart.sock"
LOG = sys.argv[2] if len(sys.argv) > 2 else "serial.log"
INJECT_AT = float(sys.argv[3]) if len(sys.argv) > 3 else 55.0

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
# 等 QEMU server 就绪
for _ in range(200):
    try:
        s.connect(SOCK)
        break
    except OSError:
        time.sleep(0.2)
else:
    print("cannot connect", file=sys.stderr)
    sys.exit(2)

stop = False
logf = open(LOG, "wb", buffering=0)

def reader():
    while not stop:
        try:
            d = s.recv(65536)
        except OSError:
            break
        if not d:
            break
        logf.write(d)

t = threading.Thread(target=reader, daemon=True)
t.start()

# 注入：root-task 绑定 pl011 IRQ 后持续注入（每 3s 一组字符）
start = time.time()
while time.time() - start < 240:
    if time.time() - start >= INJECT_AT:
        for _ in range(6):
            s.sendall(b"ENEROS-IRQ-PROBE-33")
            time.sleep(0.5)
        break
    time.sleep(0.5)

# 继续读到 240s 或断开
time.sleep(240 - (time.time() - start))
stop = True
logf.close()
s.close()
print(f"[client] injected at {INJECT_AT}s; log={LOG}")
