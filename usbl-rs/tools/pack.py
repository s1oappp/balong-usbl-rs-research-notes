#!/usr/bin/env python3
"""
pack.py — упаковка ELF usbl-rs в образ usblsafe-327x.bin (формат оригинала).

Формат (docs/00 §3, BUILD.md §1):
  файл 0x000..0x053 — заголовок bootrom (84 байта, байт-в-байт оригинал)
  файл 0x054..0x3BF — стаб @ IRAM 0x2FFE4000, 876 байт (добивка нулями)
  файл 0x3C0..EOF   — тело @ SDRAM 0x30004000, 1 753 484 байт (добивка)

Заголовок содержит: +0x28 = 0x36C (размер стаба), +0x2C = 0x2FFE4000 (вход),
+0x38 = 0x1AC18C (размер тела), +0x3C = 0x30004000 (адрес), +0x40 = 0x3C0
(смещение тела). Поскольку паковщик добивает секции РОВНО до исходных
размеров, все поля заголовка остаются валидными без изменений — итоговый
образ имеет тот же размер 1 754 444 байта и тот же заголовок, что оригинал.
"""
import struct, sys, os, hashlib

STUB_VA = 0x2FFE4000
STUB_FILE_OFF = 0x54
STUB_SIZE = 0x36C              # 876 — из заголовка оригинала (+0x28)
BODY_VA = 0x30004000
BODY_FILE_OFF = 0x3C0
BODY_SIZE = 0x1AC18C           # 1 753 484 — из заголовка оригинала (+0x38)
TOTAL_SIZE = STUB_FILE_OFF + STUB_SIZE + BODY_SIZE   # 1 754 444

def parse_elf_sections(path):
    """Минимальный парсер ELF32 little-endian: {имя: (addr, данные)}."""
    data = open(path, "rb").read()
    if data[:4] != b"\x7fELF":
        sys.exit("не ELF")
    ei_class, ei_data = data[4], data[5]
    if ei_class != 1 or ei_data != 1:
        sys.exit("ожидался ELF32 LE")
    e_shoff = struct.unpack_from("<I", data, 0x20)[0]
    e_shentsize = struct.unpack_from("<H", data, 0x2E)[0]
    e_shnum = struct.unpack_from("<H", data, 0x30)[0]
    e_shstrndx = struct.unpack_from("<H", data, 0x32)[0]

    sections = []
    for i in range(e_shnum):
        off = e_shoff + i * e_shentsize
        sh_name, sh_type, sh_flags, sh_addr, sh_offset, sh_size = struct.unpack_from(
            "<IIIIII", data, off)
        sections.append((sh_name, sh_type, sh_addr, sh_offset, sh_size))

    # имена секций
    strtab_off = sections[e_shstrndx][3]
    def name_of(sh_name):
        end = data.find(b"\x00", strtab_off + sh_name)
        return data[strtab_off + sh_name:end].decode()

    out = {}
    for sh_name, sh_type, sh_addr, sh_offset, sh_size in sections:
        if sh_type == 8:  # NOBITS
            continue
        if sh_size == 0 or sh_addr == 0:
            continue
        out[name_of(sh_name)] = (sh_addr, data[sh_offset:sh_offset + sh_size])
    return out

def main():
    elf = sys.argv[1] if len(sys.argv) > 1 else "target/armv7a-none-eabi/release/usbl-rs"
    out = sys.argv[2] if len(sys.argv) > 2 else "usblsafe-327x-rs.bin"
    header = open(os.path.join(os.path.dirname(__file__), "..", "data", "header.bin"), "rb").read()
    assert len(header) == 84, "заголовок должен быть 84 байта"

    secs = parse_elf_sections(elf)

    # --- стаб: все секции .stub* ---
    stub_parts = []
    stub_span = (STUB_VA, STUB_VA + STUB_SIZE)
    for name, (addr, blob) in sorted(secs.items(), key=lambda kv: kv[1][0]):
        if name.startswith(".stub") and addr >= STUB_VA and addr < stub_span[1]:
            stub_parts.append((addr, blob))
    if not stub_parts:
        sys.exit("нет секции .stub в ELF")

    stub = bytearray(STUB_SIZE)
    stub_end = 0
    for addr, blob in stub_parts:
        off = addr - STUB_VA
        if off + len(blob) > STUB_SIZE:
            sys.exit(f"стаб не влезает: секция @0x{addr:08X} + {len(blob)} > {STUB_SIZE}")
        stub[off:off + len(blob)] = blob
        stub_end = max(stub_end, off + len(blob))

    # --- тело: секции с адресами >= 0x30004000 ---
    body = bytearray(BODY_SIZE)
    body_end = 0
    for name, (addr, blob) in sorted(secs.items(), key=lambda kv: kv[1][0]):
        if addr >= BODY_VA and addr < BODY_VA + BODY_SIZE:
            off = addr - BODY_VA
            if off + len(blob) > BODY_SIZE:
                sys.exit(f"тело не влезает: {name} @0x{addr:08X} + {len(blob)}")
            body[off:off + len(blob)] = blob
            body_end = max(body_end, off + len(blob))

    image = bytearray(TOTAL_SIZE)
    image[0:84] = header
    image[STUB_FILE_OFF:STUB_FILE_OFF + STUB_SIZE] = stub
    image[BODY_FILE_OFF:BODY_FILE_OFF + BODY_SIZE] = body

    open(out, "wb").write(image)

    # --- отчёт ---
    orig_sha = "8e0939823f26ca09ba95335df32a8d830c8898b0a1b566c9d153237a8f346805"
    print(f"  стаб : {stub_end:4d}/{STUB_SIZE} байт занято (заполнение {100*stub_end/STUB_SIZE:.0f}%)")
    print(f"  тело : {body_end:7d}/{BODY_SIZE} байт занято (заполнение {100*body_end/BODY_SIZE:.1f}%)")
    print(f"  итог : {out} = {len(image)} байт")
    sha = hashlib.sha256(image).hexdigest()
    print(f"  SHA-256 (наш)      : {sha}")
    print(f"  SHA-256 (оригинал) : {orig_sha}")
    print(f"  Заголовок: байт-в-байт идентичен оригиналу: {'ДА' if bytes(image[:84]) == header else 'НЕТ'}")

    # проверки контракта bootrom
    w = lambda o: struct.unpack_from("<I", image, o)[0]
    checks = [
        ("+0x28 размер стаба", w(0x28), STUB_SIZE),
        ("+0x2C вход стаба", w(0x2C), STUB_VA),
        ("+0x30 смещение стаба", w(0x30), STUB_FILE_OFF),
        ("+0x38 размер тела", w(0x38), BODY_SIZE),
        ("+0x3C адрес тела", w(0x3C), BODY_VA),
        ("+0x40 смещение тела", w(0x40), BODY_FILE_OFF),
    ]
    ok = True
    for name, got, want in checks:
        mark = "OK" if got == want else "FAIL"
        if got != want:
            ok = False
        print(f"  {name}: 0x{got:08X} {'OK' if got == want else 'FAIL (ожид. 0x%08X)' % want}")
    if not ok:
        sys.exit(1)

if __name__ == "__main__":
    main()
