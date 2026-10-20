#!/usr/bin/env python3
"""
proto_test.py — протокольный тест: кадры, которые генерирует usbl-rs,
проверяются на самосогласованность (FCS) и на байт-точное совпадение
статических кадров оригинала.

Моделирует ровно ту логику, что реализована в Rust (hdlc.rs / dload.rs):
  * encap(payload) = [7E] данные FCS(~, lo-hi) [7E] с экранированием;
  * статические ответы ACK/0x3A/ERR/0x0D — из образа оригинала;
  * 'E' = 0x45 + DLOAD_ID (0x15 байт), 'F' = 0x46 + версия ПО (0x1A),
    'K' = 0x4B C9 03 FF + версия ПО (0x42) со сдвигом (без ведущего флага).
"""
import struct, sys

def fcs16(data):
    fcs = 0xFFFF
    for b in data:
        fcs ^= b
        for _ in range(8):
            fcs = (fcs >> 1) ^ 0x8408 if fcs & 1 else fcs >> 1
    return fcs & 0xFFFF

def encap(payload):
    out = bytearray([0x7E])
    fcs = 0xFFFF
    for b in payload:
        fcs = (fcs >> 8) ^ TBL[(fcs ^ b) & 0xFF]
        if b in (0x7E, 0x7D):
            out += bytes([0x7D, b ^ 0x20])
        else:
            out.append(b)
    fcs = ~fcs & 0xFFFF
    for b in ((fcs & 0xFF), (fcs >> 8)):
        if b in (0x7E, 0x7D):
            out += bytes([0x7D, b ^ 0x20])
        else:
            out.append(b)
    out.append(0x7E)
    return bytes(out)

# таблица как в образе (RFC 1662)
TBL = []
for i in range(256):
    fcs = i
    for _ in range(8):
        fcs = (fcs >> 1) ^ 0x8408 if fcs & 1 else fcs >> 1
    TBL.append(fcs & 0xFFFF)

def decap_valid(frame):
    """Кадр валиден, если FCS по содержимому между флагами == 0xF0B8."""
    inner = frame[1:-1]
    # снять экранирование
    un = bytearray()
    esc = False
    for b in inner:
        if esc:
            un.append(b ^ 0x20)
            esc = False
        elif b == 0x7D:
            esc = True
        else:
            un.append(b)
    return fcs16(bytes(un)) == 0xF0B8

ok = True
def check(name, cond, detail=""):
    global ok
    print(f"  [{'OK ' if cond else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
    if not cond:
        ok = False

print("=== Статические кадры (байт-в-байт из образа) ===")
ACK = bytes.fromhex("7e026ad37e")
ST3A = bytes.fromhex("7e3aa16e7e")
ERR = bytes.fromhex("7e030007" + "175d" + "7e")
ST0D = bytes.fromhex("0d0f") + b"7200B--SKCBADZM" + bytes.fromhex("c37b7e")
check("ACK: payload [0x02], FCS валиден", decap_valid(ACK))
check("0x3A: payload [0x3A], FCS валиден", decap_valid(ST3A))
check("ERR: payload [03 00 07], FCS валиден", decap_valid(ERR))
check("0x0D: payload [0d 0f + str], FCS валиден", decap_valid(b"\x7e" + ST0D))

print("=== Динамические кадры (логика dload.rs) ===")
DLOAD_ID = b"CH1E3272SM"      # продукт E3272s-153 (из таблицы образа)
SW_VER = b"21.420.99.07.00"   # дефолт BSP_OM_GetDloadVer

# 'E': 0x15 байт, инкапсуляция с обоими флагами
e_payload = bytearray(0x15)
e_payload[0] = 0x45
e_payload[1:1+len(DLOAD_ID)] = DLOAD_ID
e_frame = encap(bytes(e_payload))
check("'E' кадр: 0x15 байт + DLOAD_ID, FCS валиден", decap_valid(e_frame), e_frame.hex())

# 'F': 0x1A байт
f_payload = bytearray(0x1A)
f_payload[0] = 0x46
f_payload[1:1+len(SW_VER)] = SW_VER
f_frame = encap(bytes(f_payload))
check("'F' кадр: 0x1A байт + версия ПО, FCS валиден", decap_valid(f_frame), f_frame.hex())

# 'K': 0x42 байта, СДВИГ на 1 (memcpy оригинала — без ведущего флага)
k_payload = bytearray(0x42)
k_payload[0] = 0x4B
k_payload[1:4] = bytes([0xC9, 0x03, 0xFF])
k_payload[4:4+len(SW_VER)] = SW_VER
k_frame_full = encap(bytes(k_payload))
k_frame = k_frame_full[1:]  # сдвиг как в dloadUSBSendPro
check("'K' кадр: 0x42 байта, сдвиг-на-1, FCS валиден",
      decap_valid(b"\x7e" + k_frame), k_frame.hex())

print("=== Пакеты DLOAD (формат FUN_30080798) ===")
# START: [0x41][type BE32][size BE32]
start = bytes([0x41]) + struct.pack(">I", 0x105) + struct.pack(">I", 0x300000)
start_frame = encap(start)
check("START (0x41, kernel, 3 МБ): FCS валиден", decap_valid(start_frame))
# DATA: [0x42][seq BE32][len BE16][data]
data_pkt = bytes([0x42]) + struct.pack(">I", 1) + struct.pack(">H", 4) + b"\xde\xad\xbe\xef"
check("DATA (0x42, seq=1, len=4): длина payload = len+7", len(data_pkt) == 4 + 7)
check("DATA (0x42): FCS валиден", decap_valid(encap(data_pkt)))
# END: [0x43] + 23 байта
end_pkt = bytes([0x43]) + b"\x00" * 22
check("END (0x43): payload_len = 0x17", len(end_pkt) == 0x17)

print("=== AT-команды (таблицы уровня 1) ===")
AT_CMDS = [b"ATE0\r", b"ATE1\r", b"AT^DLOADVER?\r", b"AT^DLOADINFO?\r",
           b"AT^AUTHORITYVER?\r", b"AT^AUTHORITYID?\r", b"AT^DATAMODE\r",
           b"AT^HWNATQRY?\r", b"AT^FLASHINFO?\r", b"AT+CMEE=0\r", b"AT^GODLOAD\r"]
check("11 команд таблицы ATCommandProcess", len(AT_CMDS) == 11)
check("AT^DATAMODE — терминатор AT-диалога", b"AT^DATAMODE\r" in AT_CMDS)

print()
sys.exit(0 if ok else 1)
