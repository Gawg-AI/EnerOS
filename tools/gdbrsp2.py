#!/usr/bin/env python3
"""QEMU gdbstub：硬件断点 + 单步跟踪用户执行流（诊断 EL0 syscall 错乱）。"""
import socket, sys, time

HOST, PORT = '127.0.0.1', 1234

class Gdb:
    def __init__(self):
        self.s = socket.create_connection((HOST, PORT), timeout=60)
        self.buf = b''
    def send(self, data: bytes):
        msg = b'$' + data + b'#'
        cksum = sum(data) & 0xFF
        msg += b'%02x' % cksum
        self.s.sendall(msg)
    def recv_packet(self) -> bytes:
        while True:
            i = self.buf.find(b'$')
            if i >= 0:
                j = self.buf.find(b'#', i)
                if j >= 0 and j + 2 < len(self.buf):
                    pkt = self.buf[i+1:j]
                    self.buf = self.buf[j+3:]
                    return pkt
            data = self.s.recv(4096)
            if not data:
                raise EOFError('closed')
            self.buf += data
    def cmd(self, data: bytes) -> bytes:
        self.send(data)
        while True:
            pkt = self.recv_packet()
            if pkt in (b'OK', b''):
                return pkt
            if pkt.startswith(b'E'):
                return pkt
            if pkt.startswith(b'T') or pkt.startswith(b'S'):
                return pkt
            return pkt
    def cont(self):
        self.send(b'c')
        return self.recv_packet()
    def step(self):
        self.send(b's')
        return self.recv_packet()
    def hw_bp(self, addr: int):
        a = ('%x' % addr).encode()
        return self.cmd(b'Z1,' + a + b',4')
    def p(self, expr: str):
        r = self.cmd(b'p' + expr.encode())
        if r.startswith(b'$'):
            r = r[1:]
        return r
    def read_mem(self, addr: int, n: int):
        a = ('%x' % addr).encode()
        l = ('%x' % n).encode()
        return self.cmd(b'm' + a + b',' + l)

def main():
    g = Gdb()
    g.cmd(b'?')
    print('bp 0x400ff8:', g.hw_bp(0x400ff8))
    print('bp 0x401e94:', g.hw_bp(0x401e94))
    print('bp 0x401ec4:', g.hw_bp(0x401ec4))
    deadline = time.time() + 150
    hits = 0
    try:
        while time.time() < deadline:
            pkt = g.cont()
            if pkt.startswith(b'T'):
                pc = g.p('$pc').decode()
                x8 = g.p('$x8').decode()
                x30 = g.p('$x30').decode()
                x0 = g.p('$x0').decode()
                print(f'HIT pc={pc} x0={x0} x8={x8} x30={x30}')
                hits += 1
                # 命中 0x400ff8 后单步 40 条，观察执行流。
                if pc == '0x400ff8' and hits >= 1:
                    for i in range(40):
                        g.step()
                        pc2 = g.p('$pc').decode()
                        x82 = g.p('$x8').decode()
                        x302 = g.p('$x30').decode()
                        print(f'  step{i}: pc={pc2} x8={x82} x30={x302}')
                        if pc2 in ('0x401e94', '0x401ec4', '0x401ef4'):
                            break
                    break
            elif pkt.startswith(b'S'):
                print('EXITED:', pkt)
                break
            else:
                print('PKT:', pkt)
    except Exception as e:
        print('ERR:', e)
    print('done hits=', hits)

if __name__ == '__main__':
    main()
