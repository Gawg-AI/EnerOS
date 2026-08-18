import socket, sys, time, struct, select
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(15)
deadline = time.time() + 12
while True:
    try:
        s.connect(('127.0.0.1', 5577)); break
    except OSError:
        if time.time() > deadline: print("PEER: connect timeout"); sys.exit(1)
        time.sleep(0.1)
s.settimeout(0.5)
frame = bytes.fromhex('020000000001') + bytes.fromhex('525400123456') + b'\x88\xb5' + b'ENEROS-NET-RX'
# 36 收尾加固：周期重发（root 的 vnet_rx 只等约 2s，单次突发可能错过），
# 直到收到 root 的 TX 回应或超时；收到即停止重发。
data = b''
t0 = time.time()
got = False
last_send = 0.0
while time.time() - t0 < 20 and not got:
    if time.time() - last_send >= 1.0:
        try:
            s.sendall(struct.pack('!I', len(frame)) + frame)
        except OSError:
            break
        last_send = time.time()
    r, _, _ = select.select([s], [], [], 0.5)
    if r:
        try:
            chunk = s.recv(4096)
        except OSError:
            break
        if not chunk: break
        data += chunk
        # 尝试解析 [len(4) + payload]，payload 剥残留
        off = 0
        while off + 4 <= len(data):
            plen = struct.unpack('!I', data[off:off+4])[0]
            if plen <= 0 or plen > 2048:
                break
            payload = data[off+4:off+4+plen]
            for skip in (0, 2, 4):
                fr = payload[skip:]
                if len(fr) >= 27 and fr[12:14] == b'\x88\xb5' and fr[14:27] == b'ENEROS-NET-TX':
                    got = True
                    print("PEER: matched plen=%d skip=%d" % (plen, skip))
                    break
            if got: break
            off += 4 + plen
        if got: break
print("PEER RX total=%d hex=%s" % (len(data), data.hex()))
sys.exit(0 if got else 2)
