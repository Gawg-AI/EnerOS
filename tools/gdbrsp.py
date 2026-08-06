#!/usr/bin/env python3
"""最小 GDB RSP 客户端：在指定地址设断点，跟踪命中顺序。"""
import socket, sys, time, struct

HOST, PORT = '127.0.0.1', 1234

class Gdb:
    def __init__(self):
        self.s = socket.create_connection((HOST, PORT), timeout=30)
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
            if pkt == b'OK' or pkt.startswith(b'E'):
                return pkt
            if pkt.startswith(b'T') or pkt.startswith(b'S'):
                return pkt
            if pkt == b'':
                continue
            return pkt
    def cont(self):
        self.send(b'c')
        return self.recv_packet()
    def set_bp(self, addr: int):
        a = ('%x' % addr).encode()
        return self.cmd(b'Z0,' + a + b',4')
    def regs(self):
        return self.cmd(b'g')
    def reg(self, n):
        r = self.regs()
        off = n * 16
        return int(r[off:off+16], 16)

def main():
    g = Gdb()
    g.cmd(b'?')
    bps = {
        0x400ff8: 'after-start-debug (abi_version call)',
        0x401e6c: 'abi_version func',
        0x401ecc: 'process_exit func',
        0x40109c: 'version-check',
    }
    for a in bps:
        r = g.set_bp(a)
        print(f'bp {hex(a)}: {r}')
    hits = []
    deadline = time.time() + 120
    try:
        while time.time() < deadline:
            pkt = g.cont()
            if pkt.startswith(b'T'):
                pc = g.reg(32)
                x30 = g.reg(30)
                x0 = g.reg(0)
                x8 = g.reg(8)
                desc = bps.get(pc, f'pc={hex(pc)}')
                print(f'HIT {hex(pc)} {desc} x0={hex(x0)} x8={hex(x8)} x30={hex(x30)}')
                hits.append(pc)
                if len(hits) > 12:
                    break
            elif pkt.startswith(b'S'):
                print('PROGRAM EXITED:', pkt)
                break
            else:
                print('PKT:', pkt)
                if pkt == b'':
                    break
    except Exception as e:
        print('ERR:', e)
    print('HITS:', [hex(h) for h in hits])

if __name__ == '__main__':
    main()
