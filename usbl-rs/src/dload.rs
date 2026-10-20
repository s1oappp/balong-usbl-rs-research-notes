//! dload.rs — DLOAD-протокол (пакетный, поверх HDLC) — главный процессор.
//!
//! РЕКОНСТРУКЦИЯ FUN_30080798 @0x30080798 (5352 б) + dloadUSBSendPro
//! @0x3007FE10 (2148 б):
//!
//! Формат пакета (полезная часть HDLC-кадра, байты после decap):
//!   * команда — байт 0;
//!   * поля big-endian (HighToLow):
//!       0x41 'A' START : [0x41][type:4 BE][total_size:4 BE] —
//!                        imageTypeChange + dloadInitRamBuffer;
//!       0x42 'B' DATA  : [0x42][seq:4 BE][len:2 BE][data:len] —
//!                        проверка payload_len == len+7; seq: ожидается
//!                        prev+1 (новый) или prev (повтор), иначе ошибка;
//!       0x43 'C' END   : payload_len == 0x17 (23) — финализация, запись
//!                        маркер-файлов для cdromiso(0x10C)/0x65, DCBInit;
//!       0x0A           : конец сессии — ACK, dloadBootromEnd(0x20), ребут;
//!       0x0C           : отмена — сброс состояния (LED, HdlcInit 0x2200);
//!       0x3A, 0x45 'E', 0x46 'F', 0x4B 'K', 0xFB — статусные/версионные;
//!       прочее → код 0x306 без ответа.
//!   * после каждой команды — dloadUSBSendPro(status):
//!       2 → стат. кадр «7e 02 6a d3 7e» (ACK);
//!       0x3A → «7e 3a a1 6e 7e»;
//!       0x0D → «0d 0f „7200B--SKCBADZM“ c3 7b 7e» (20 байт);
//!       0x45 → 'E' + DLOAD_ID (0x15 байт, HDLC-инкапсуляция);
//!       0x46 → 'F' + версия ПО (0x1A байт);
//!       0x4B → 'K' 0xC9 0x03 0xFF + версия ПО (0x42 байта, инкапсуляция
//!              с удалением ведущего флага — memcpy-сдвиг оригинала);
//!       ошибки 0x303/0x307 → без ответа (таймаут хоста).

use crate::hdlc::{self, DecapResult, HdlcState};
use crate::mmio::delay_ms;

/// Буфер приёма HDLC (0x2200 — как FUN_30080798: Om_HdlcInit(...; 0x2200)).
static mut RX_BUF: [u8; hdlc::HDLC_MAX_FRAME] = [0; hdlc::HDLC_MAX_FRAME];
/// Состояние декапсулятора.
static mut HDLC: Option<HdlcState> = None;
/// Флаг data-mode (AT^DATAMODE / AT^GODLOAD).
static mut DATA_MODE: bool = false;
/// Флаг AT^GODLOAD — немедленный уход в DLOAD.
static mut GO_DLOAD: bool = false;

/// Состояние сессии загрузки (аналог DCB/текста FUN_30080798):
static mut SESSION: Option<Session> = None;

#[derive(Clone, Copy)]
struct Session {
    /// Тип образа (imageTypeChange, +4 в структуре оригинала).
    image_type: u32,
    /// Общий размер образа (+8).
    total_size: u32,
    /// Принято байт.
    received: u32,
    /// Ожидаемый порядковый номер пакета 0x42 (+0x20).
    expect_seq: u32,
    /// Смещение записи во flash (производная ptable).
    write_offset: u32,
    /// Буфер образа (в SDRAM, поверх зоны тела — как dloadInitRamBuffer).
    buf: *mut u8,
    buf_len: usize,
}

/// Буфер образа: располагается в SDRAM за телом загрузчика
/// (dloadInitRamBuffer выделяет его malloc'ом; здесь — статически).
static mut IMAGE_BUF: [u8; IMAGE_BUF_SIZE] = [0; IMAGE_BUF_SIZE];
/// Максимальный образ: ядро 3 МБ (kernel @0x3300000). Больше в burn-режим
/// не льётся (VxWorks 32 МБ — через dinFS-путь, здесь ограничено памятью).
pub const IMAGE_BUF_SIZE: usize = 3 * 1024 * 1024;

/// Инициализация подсистемы (dloadInit @0x30076BA8: DCBInit, Om_HdlcInit,
/// dload_nand_init, задача tDloadTask — здесь всё в главном цикле).
pub fn init() {
    unsafe {
        HDLC = Some(HdlcState::new(RX_BUF.as_mut_ptr(), hdlc::HDLC_MAX_FRAME));
    }
}

/// Установка data-mode (atSetDataModeBr).
pub fn set_data_mode(on: bool) {
    unsafe {
        DATA_MODE = on;
    }
}

/// Флаг AT^GODLOAD.
pub fn set_go_dload() {
    unsafe {
        GO_DLOAD = true;
    }
}

/// Есть ли data-mode?
pub fn data_mode() -> bool {
    unsafe { DATA_MODE }
}

/// Большой endian read (HighToLow оригинала).
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Кормление байтами из USB (dloadUSBCallback: байт-за-байтом в decap).
pub fn feed(bytes: &[u8]) {
    unsafe {
        let hdlc = match HDLC.as_mut() {
            Some(h) => h,
            None => return,
        };
        for &b in bytes {
            match hdlc.decap(b) {
                DecapResult::FrameReady => {
                    // кадр готов — FUN_30080798()
                    let _ = process_packet(hdlc.frame());
                }
                DecapResult::InProgress => {}
                _ => {
                    // ошибки CRC/escape/overflow — кадр сброшен декапом
                }
            }
        }
    }
}

/// Главный процессор пакета (FUN_30080798). Возвращает код для ответа.
fn process_packet(payload: &[u8]) -> u32 {
    if payload.is_empty() {
        return 0x306; // DAT_30081e70 — без ответа
    }
    let cmd = payload[0];
    let plen = payload.len();

    match cmd {
        0x0A => {
            // конец сессии: ACK, пауза, финализация + мягкая перезагрузка
            send_pro(2);
            delay_ms(2000); // taskDelay(200) x 10мс
            dload_bootrom_end(0x20);
            return 0x310; // (оригинал возвращает 0x310 — уже после ребута)
        }
        0x0C => {
            // отмена: сброс состояния сессии, ACK
            unsafe {
                SESSION = None;
            }
            send_pro(0x0D);
            return 0x0D;
        }
        0x3A => {
            send_pro(0x3A);
            return 0x3A;
        }
        0x41 => {
            // START: [41][type:4][size:4]
            if plen < 9 {
                return 0x307;
            }
            let image_type = be32(&payload[1..5]);
            let total = be32(&payload[5..9]);
            if !image_type_change(image_type) {
                return 0x307;
            }
            unsafe {
                SESSION = Some(Session {
                    image_type,
                    total_size: total,
                    received: 0,
                    expect_seq: 0,
                    write_offset: 0,
                    buf: IMAGE_BUF.as_mut_ptr(),
                    buf_len: IMAGE_BUF_SIZE,
                });
                // подготовка места во flash (ptable_flash_prepare)
                if let Some((_, e)) = crate::ptable::find_older_part(image_type) {
                    crate::nand::flash_prepare(e.offset, e.size);
                }
            }
            send_pro(2);
            return 2;
        }
        0x42 => {
            // DATA: [42][seq:4][len:2][data:len]
            if plen < 7 {
                return 0x303;
            }
            let seq = be32(&payload[1..5]);
            let dlen = be16(&payload[5..7]) as usize;
            if plen != dlen + 7 {
                // несоответствие длины — код ошибки 0x304
                send_pro(0x304);
                return 0x304;
            }
            let s = unsafe { SESSION.as_mut() };
            let s = match s {
                Some(s) => s,
                None => return 0x303,
            };
            if seq == s.expect_seq {
                // повтор пакета — просто ACK
                send_pro(2);
                return 2;
            }
            if seq != s.expect_seq.wrapping_add(1) {
                // потеря пакета — 0x304
                send_pro(0x304);
                return 0x304;
            }
            // принять данные
            let off = s.received as usize;
            if off + dlen > s.buf_len {
                send_pro(0x307);
                return 0x307;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(payload[7..].as_ptr(), s.buf.add(off), dlen);
            }
            s.received += dlen as u32;
            s.expect_seq = seq;
            send_pro(2);
            return 2;
        }
        0x43 => {
            // END: финализация (payload_len == 0x17 в оригинале)
            if plen != 0x17 {
                return 0x306;
            }
            let s = unsafe { SESSION.take() };
            let s = match s {
                Some(s) => s,
                None => return 0x306,
            };
            // запись образа во flash (imageDloadProcess → ImageDloadToFlash)
            let ok = image_dload_to_flash(&s);
            if ok {
                // маркер-файлы для cdromiso/ISO-типов — вне объёма (ФС);
                // финализация ptable
                send_pro(2);
                2
            } else {
                send_pro(0x307);
                0x307
            }
        }
        0x45 => {
            // 'E': DLOAD_ID
            send_pro(0x45);
            0x45
        }
        0x46 => {
            // 'F': версия ПО
            send_pro(0x46);
            0x46
        }
        0x4B => {
            // 'K': полная версия ПО
            send_pro(0x4B);
            0x4B
        }
        0xFB => {
            send_pro(0xFB);
            0xFB
        }
        _ => 0x306,
    }
}

/// imageTypeChange @0x300835A8-подобная проверка типа образа.
fn image_type_change(kind: u32) -> bool {
    match kind {
        0x100 => true,                                        // новая ptable
        x if (0x101..=0x113).contains(&x) => true,            // разделы
        0x65 => true,                                         // ISO-образ
        _ => false,
    }
}

/// Запись образа в старую копию раздела (imageDloadProcess →
/// ImageDloadToFlash → ptable_find_older_part → nand_write).
fn image_dload_to_flash(s: &Session) -> bool {
    let data = unsafe { core::slice::from_raw_parts(s.buf, s.received as usize) };
    if s.image_type == 0x100 {
        // новая ptable: merge в RAM-таблицу (ptable_replace_ram_table)
        let t = crate::ptable::ram_table();
        let n = data.len().min(t.len());
        unsafe {
            let mut ram = crate::ptable::ram_table_mut();
            ram[..n].copy_from_slice(&data[..n]);
        }
        return crate::ptable::replace_flash_table();
    }
    match crate::ptable::find_older_part(s.image_type) {
        Some((idx, e)) => {
            crate::console::puts("\r\nUSBL: запись раздела ");
            crate::console::write_bytes(e.name_bytes());
            crate::console::puts(" (");
            crate::console::put_u32_dec(s.received);
            crate::console::puts(" байт)...");
            let ok = crate::nand::write(e.offset, data);
            if ok {
                crate::ptable::activate_copy(idx);
                crate::console::putln(" OK");
            } else {
                crate::console::putln(" ОШИБКА");
            }
            ok
        }
        None => false,
    }
}

/// Финализация сессии (dloadBootromEnd @0x30076EC0):
/// ptable_replace_flash_table → set_integrality → SetSoftLoad → Reset.
fn dload_bootrom_end(_code: u32) {
    crate::ptable::replace_flash_table();
    crate::ptable::set_integrality(true);
    crate::safety::set_marker(); // контракт слова 0x2ffffffc перед ребутом
    crate::console::putln("USBL: сессия завершена, перезагрузка");
    delay_ms(300);
    crate::wdt::reboot_direct();
}

/// Отправка ответа (dloadUSBSendPro @0x3007FE10 — свёртка по статусу).
pub fn send_pro(status: u32) {
    match status {
        2 => send_frame_raw(&[
            0x7E, 0x02, 0x6A, 0xD3, 0x7E, // DAT_30080da8 (байт-в-байт)
        ]),
        0x0D => {
            // DAT_30080dc0: 20 байт, ведущий флаг ОТСУТСТВУЕТ (как в образе)
            let mut frame = [0u8; 20];
            frame[0] = 0x0D;
            frame[1] = 0x0F;
            let s = b"7200B--SKCBADZM";
            frame[2..2 + s.len()].copy_from_slice(s);
            frame[17] = 0xC3; // FCS lo
            frame[18] = 0x7B; // FCS hi
            frame[19] = 0x7E; // закрывающий флаг
            send_frame_raw(&frame);
        }
        0x3A => send_frame_raw(&[0x7E, 0x3A, 0xA1, 0x6E, 0x7E]),
        0x45 => {
            // 'E' + DLOAD_ID, 0x15 байт, инкапсуляция с обоими флагами
            let id = crate::version::dload_id();
            let mut payload = [0u8; 0x15];
            payload[0] = 0x45;
            let n = id.len().min(0x14);
            payload[1..1 + n].copy_from_slice(&id[..n]);
            send_encapsulated(&payload);
        }
        0x46 => {
            // 'F' + версия ПО (0x1A байт)
            let v = crate::version::software_ver();
            let mut payload = [0u8; 0x1A];
            payload[0] = 0x46;
            let n = v.len().min(0x19);
            payload[1..1 + n].copy_from_slice(&v[..n]);
            send_encapsulated(&payload);
        }
        0x4B => {
            // 'K' [C9 03 FF] + версия ПО (0x42 байта), СДВИГ на 1 (memcpy)
            let v = crate::version::software_ver();
            let mut payload = [0u8; 0x42];
            payload[0] = 0x4B;
            payload[1] = 0xC9;
            payload[2] = 0x03;
            payload[3] = 0xFF;
            let n = v.len().min(0x3E);
            payload[4..4 + n].copy_from_slice(&v[..n]);
            let mut frame = [0u8; 0x100];
            if let Some(n) = hdlc::encap(&payload, &mut frame) {
                // оригинал: memcpy(dst, dst+1, len); send(dst, len-1)
                // — удаляем ВЕДУЩИЙ флаг перед отправкой
                send_frame_raw(&frame[1..n]);
            }
        }
        _ => {} // 0x303/0x306/0x307 — без ответа (таймаут хоста)
    }
}

/// Инкапсулированный кадр с обоими флагами (как 'E'/'F' оригинала).
fn send_encapsulated(payload: &[u8]) {
    let mut frame = [0u8; 0x100];
    if let Some(n) = hdlc::encap(payload, &mut frame) {
        send_frame_raw(&frame[..n]);
    }
}

/// Отправка сырого кадра через USB (dloadUSBSend → BSP_DLOAD_SendData).
fn send_frame_raw(frame: &[u8]) {
    crate::usb::dload_send(frame);
}
