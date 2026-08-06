#!/usr/bin/env python3
"""生成 fuzz 语料种子（v0.242.0 B6：6 个解析面补种子 + 往返/差分断言）。

每种子的文件名 = 内容 sha256 前 16 位，与 libFuzzer 语料惯例一致。
"""
import hashlib, os, struct

BASE = os.path.join(os.path.dirname(__file__), "corpus")
os.makedirs(BASE, exist_ok=True)

def emit(kind, data):
    d = os.path.join(BASE, kind)
    os.makedirs(d, exist_ok=True)
    name = hashlib.sha256(data).hexdigest()[:16]
    with open(os.path.join(d, name), "wb") as f:
        f.write(data)

# ---- ipc_wire: El0Frame 有效帧 ----
def ipc_wire():
    # magic(4) + ver(1) + type(1) + payload_len(2) + payload
    for ptype in (1, 2, 3):
        payload = b"ping".ljust(32, b"\0")
        frame = b"EL0F" + bytes([1, ptype]) + struct.pack("<H", len(payload)) + payload
        emit("ipc_wire", frame)
    # 空 payload
    emit("ipc_wire", b"EL0F" + bytes([1, 1]) + struct.pack("<H", 0))

# ---- boot_protocol: 组件清单/验签头 ----
def boot_protocol():
    # ESIG 签名头（HEADER_LEN=64 惯例）
    for size in (32, 64, 128):
        hdr = b"ESIG" + struct.pack("<H", 1) + struct.pack("<Q", size)
        hdr += bytes(32) + bytes(64) + struct.pack("<Q", 0)
        emit("boot_protocol", hdr)
    # 带 payload 的完整镜像
    img = b"ESIG" + struct.pack("<H", 1) + struct.pack("<Q", 16)
    img += bytes(32) + bytes(64) + struct.pack("<Q", 0) + b"\x55" * 16
    emit("boot_protocol", img)

# ---- elf_loader: 最小合法 ELF64 头 ----
def elf_loader():
    # ELF64 头（64 字节）
    eh = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    eh += struct.pack("<HHIQQQIHHHHHH", 2, 62, 1, 0x400000, 0, 0, 0, 64, 0, 0, 64, 0, 0)
    emit("elf_loader", eh)
    # ELF32
    eh32 = b"\x7fELF" + bytes([1, 1, 1, 0]) + bytes(8)
    eh32 += struct.pack("<HHIIIIIHHHHHH", 2, 3, 1, 0x8000, 0, 0, 0, 52, 0, 0, 52, 0, 0)
    emit("elf_loader", eh32)

# ---- gguf_loader: 最小 GGUF 头（magic+version+tensor_count） ----
def gguf_loader():
    # GGUF magic "GGUF" + version u32 + tensor_count u64
    hdr = b"GGUF" + struct.pack("<I", 3) + struct.pack("<Q", 0)
    emit("gguf_loader", hdr)
    hdr2 = b"GGUF" + struct.pack("<I", 2) + struct.pack("<Q", 1) + struct.pack("<Q", 0)
    emit("gguf_loader", hdr2)

# ---- iec104: 有效 APDU ----
def iec104():
    # 0x68 len 格式（len ≥ 4）
    emit("iec104", b"\x68\x04\x01\x00\x00\x00")
    # I 帧（带 ASDU）
    emit("iec104", b"\x68\x0e\x08\x00\x00\x00\x64\x01\x06\x00\x01\x00\x00\x00\x01\x00")
    # S 帧
    emit("iec104", b"\x68\x04\x01\x00\x00\x00")
    # U 帧 STARTDT
    emit("iec104", b"\x68\x04\x07\x00\x00\x00")

# ---- modbus_rtu: 有效 RTU 帧（addr+func+data+crc16） ----
def modbus_rtu():
    def crc16(data):
        crc = 0xFFFF
        for b in data:
            crc ^= b
            for _ in range(8):
                crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
        return crc
    for func, payload in [(0x03, bytes([0x00, 0x00, 0x00, 0x02])),
                          (0x06, bytes([0x00, 0x01, 0x00, 0x02])),
                          (0x10, bytes([0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x0A]))]:
        body = bytes([0x01, func]) + payload
        emit("modbus_rtu", body + struct.pack("<H", crc16(body)))
    # 异常响应
    emit("modbus_rtu", bytes([0x01, 0x83, 0x02]) + struct.pack("<H", crc16(bytes([0x01, 0x83, 0x02]))))

ipc_wire()
boot_protocol()
elf_loader()
gguf_loader()
iec104()
modbus_rtu()
print("seeds generated")
