import socket, sys, time
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(15)
deadline = time.time() + 12
while True:
    try:
        s.connect(('127.0.0.1', 5577)); break
    except OSError:
        if time.time() > deadline: print("PEER: connect timeout"); sys.exit(1)
        time.sleep(0.1)
s.settimeout(15)
data = b''
try:
    while len(data) < 28:
        chunk = s.recv(4096)
        if not chunk: break
        data += chunk
    print("PEER RX len=%d hex=%s" % (len(data), data.hex()))
    if len(data) >= 28 and data[12:14] == b'\x88\xb5' and data[14:28] == b'ENEROS-NET-TX':
        reply = bytes.fromhex('020000000001') + bytes.fromhex('525400123456') + b'\x88\xb5' + b'ENEROS-NET-RX'
        s.sendall(reply)
        print("PEER TX ok")
    else:
        print("PEER: unexpected frame")
        sys.exit(2)
except socket.timeout:
    print("PEER: recv timeout")
    sys.exit(3)
