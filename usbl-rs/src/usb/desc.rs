//! usb/desc.rs — дескрипторы USB burn-профиля (VID 0x12D1, PID 0x1C05).
//!
//! РЕКОНСТРУКЦИЯ ИЗ ОБРАЗА:
//!   * шаблоны дескрипторов @0x301813EC (device/qualifier/config/interface) —
//!     встроены ниже байт-в-байт как основа (bcdUSB=0200, класс 0, maxpkt0=8
//!     в шаблоне — рантайм подставляет HS 64);
//!   * burn-профиль @0x301815BC: DevIdx=0, FdCount=3, PID **0x1C05**,
//!     OSType 0x00808080, fdTypes 6,6,1,6,2,6 → 3 порта CDC-ACM
//!     «MDM+PCUI+DIAG» (строка образа «MDM+PCUI+DIAG in Bootrom image»);
//!   * VID 0x12D1 вставляется рантаймом (BSP_USB_UpdateDevPidInfo);
//!   * строки: «HUAWEI Technology» @0x3013BE48, «HUAWEI Mobile» @0x3013BE38.
//!
//! Конфигурация: 6 интерфейсов (3 x CDC-ACM: comm+data), IAD на каждый
//! порт — стандартная композитная раскладка Huawei-модемов (хост видит
//! /dev/ttyUSB0..2 через cdc_acm).

/// VID Huawei.
pub const VID: u16 = 0x12D1;
/// PID burn-режима E3272s-153.
pub const PID: u16 = 0x1C05;
/// Число CDC-ACM портов (FdCount профиля).
pub const ACM_PORTS: usize = 3;

/// Шаблон DEVICE-дескриптора из образа @0x301813EC (18 байт) — основа;
/// VID/PID/bcdDevice подставляются, bMaxPacketSize0 = 64 для HS.
pub fn device_descriptor(hs: bool) -> [u8; 18] {
    let mut d = [0u8; 18];
    d[0] = 18; // bLength
    d[1] = 0x01; // DEVICE
    d[2] = 0x00;
    d[3] = 0x02; // bcdUSB 0200
    d[4] = 0x00; // класс определяется интерфейсами (как шаблон образа)
    d[5] = 0x00;
    d[6] = 0x00;
    d[7] = 64; // bMaxPacketSize0 (HS; в FS-шаблоне образа 8)
    d[8] = (VID & 0xFF) as u8;
    d[9] = (VID >> 8) as u8;
    d[10] = (PID & 0xFF) as u8;
    d[11] = (PID >> 8) as u8;
    d[12] = 0x00; // bcdDevice — рантайм (в образе 0)
    d[13] = 0x00;
    d[14] = 1; // iManufacturer
    d[15] = 2; // iProduct
    d[16] = 0; // iSerial
    d[17] = 1; // bNumConfigurations
    let _ = hs;
    d
}

/// DEVICE QUALIFIER из образа @0x301813FE (10 байт, байт-в-байт):
/// `0a 06 00 02 00 00 00 08 01 00` — bcdUSB 0200, bMaxPacketSize0=8,
/// 1 конфигурация. Для HS-работы поле 7 (maxpkt0) = 64.
pub fn qualifier_descriptor() -> [u8; 10] {
    let mut q = [0u8; 10];
    q[0] = 10;
    q[1] = 0x06;
    q[2] = 0x00;
    q[3] = 0x02;
    q[4..7].copy_from_slice(&[0, 0, 0]);
    q[7] = 64;
    q[8] = 0x01;
    q[9] = 0x00;
    q
}

/// Строковые дескрипторы (LANGID + два из образа).
pub fn string_descriptor(index: u8) -> Option<[u8; 64]> {
    let mut buf = [0u8; 64];
    let mut o = 0usize;
    let mut push = |buf: &mut [u8], o: &mut usize, s: &[u8]| {
        for &b in s {
            if *o + 1 < buf.len() {
                buf[*o] = 0;
                buf[*o + 1] = b;
                *o += 2;
            }
        }
    };
    match index {
        0 => {
            // LANGID: только английский (0x0409)
            buf[0] = 4;
            buf[1] = 0x03;
            buf[2] = 0x09;
            buf[3] = 0x04;
            let mut r = buf;
            r[0] = 4;
            r[1] = 0x03;
            return Some(r);
        }
        1 => {
            push(&mut buf, &mut o, b"HUAWEI Technology"); // @0x3013BE48
        }
        2 => {
            push(&mut buf, &mut o, b"HUAWEI Mobile"); // @0x3013BE38
        }
        _ => return None,
    }
    buf[0] = (o + 2) as u8;
    buf[1] = 0x03;
    Some(buf)
}

/// Полный конфигурационный дескриптор: 3 x CDC-ACM (IAD + comm + data).
/// Размер: 9 + 3 x (8 + 9 + 5 + 5 + 4 + 5 + 7 + 9 + 7 + 7) = 9 + 3x66 = 207.
pub fn config_descriptor() -> [u8; 256] {
    let mut c = [0u8; 256];
    let mut o = 0usize;

    // CONFIGURATION (9)
    c[o] = 9;
    c[o + 1] = 0x02;
    let total: u16 = (9 + ACM_PORTS as u16 * 66) as u16;
    c[o + 2] = (total & 0xFF) as u8;
    c[o + 3] = (total >> 8) as u8;
    c[o + 4] = (ACM_PORTS * 2) as u8; // 6 интерфейсов
    c[o + 5] = 1; // bConfigurationValue
    c[o + 6] = 0; // iConfiguration
    c[o + 7] = 0x80; // bus-powered (как шаблон образа)
    c[o + 8] = 0xFA; // 500 мА (как шаблон образа)
    o += 9;

    for port in 0..ACM_PORTS {
        let comm_if = (port * 2) as u8;
        let data_if = (port * 2 + 1) as u8;

        // IAD (8)
        c[o] = 8;
        c[o + 1] = 0x0B;
        c[o + 2] = comm_if;
        c[o + 3] = 2; // 2 интерфейса в функции
        c[o + 4] = 0x02; // класс CDC
        c[o + 5] = 0x02; // подкласс ACM
        c[o + 6] = 0x01; // протокол AT (V.25ter)
        c[o + 7] = 0; // iFunction
        o += 8;

        // INTERFACE comm (9)
        c[o] = 9;
        c[o + 1] = 0x04;
        c[o + 2] = comm_if;
        c[o + 3] = 0; // alt 0
        c[o + 4] = 1; // 1 эндпоинт (notify)
        c[o + 5] = 0x02;
        c[o + 6] = 0x02;
        c[o + 7] = 0x01;
        c[o + 8] = 0;
        o += 9;

        // CDC Header (5)
        c[o] = 5;
        c[o + 1] = 0x24;
        c[o + 2] = 0x00;
        c[o + 3] = 0x10;
        c[o + 4] = 0x01;
        o += 5;

        // CDC Call Management (5)
        c[o] = 5;
        c[o + 1] = 0x24;
        c[o + 2] = 0x01;
        c[o + 3] = 0x00; // сами обрабатываем
        c[o + 4] = data_if;
        o += 5;

        // CDC ACM (4)
        c[o] = 4;
        c[o + 1] = 0x24;
        c[o + 2] = 0x02;
        c[o + 3] = 0x02; // поддерживаем SET_LINE_CODING + SERIAL_STATE
        o += 4;

        // CDC Union (5)
        c[o] = 5;
        c[o + 1] = 0x24;
        c[o + 2] = 0x06;
        c[o + 3] = comm_if;
        c[o + 4] = data_if;
        o += 5;

        // ENDPOINT notify IN (7) — EP (port+4), INT
        c[o] = 7;
        c[o + 1] = 0x05;
        c[o + 2] = (0x80 | ((port as u8) + 4)) as u8;
        c[o + 3] = 0x03;
        c[o + 4] = 0x40; // 64 байта
        c[o + 5] = 0x00;
        c[o + 6] = 0x10; // 16 мс
        o += 7;

        // INTERFACE data (9)
        c[o] = 9;
        c[o + 1] = 0x04;
        c[o + 2] = data_if;
        c[o + 3] = 0;
        c[o + 4] = 2;
        c[o + 5] = 0x0A; // CDC-Data
        c[o + 6] = 0x00;
        c[o + 7] = 0x00;
        c[o + 8] = 0;
        o += 9;

        // ENDPOINT bulk OUT (7) — EP (port+1)
        c[o] = 7;
        c[o + 1] = 0x05;
        c[o + 2] = ((port as u8) + 1) & 0x0F;
        c[o + 3] = 0x02;
        c[o + 4] = 0x00;
        c[o + 5] = 0x02; // 512 (HS)
        c[o + 6] = 0x00;
        o += 7;

        // ENDPOINT bulk IN (7) — EP (port+1)
        c[o] = 7;
        c[o + 1] = 0x05;
        c[o + 2] = (0x80 | ((port as u8) + 1)) as u8;
        c[o + 3] = 0x02;
        c[o + 4] = 0x00;
        c[o + 5] = 0x02;
        c[o + 6] = 0x00;
        o += 7;
    }
    c
}

/// Длина конфигурации (wTotalLength).
pub const CONFIG_TOTAL_LEN: usize = 9 + ACM_PORTS * 66;
