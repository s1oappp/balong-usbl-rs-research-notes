//! at.rs — AT-команды загрузчика (таблицы байт-в-байт из образа).
//!
//! РЕКОНСТРУКЦИЯ (docs/04 §2):
//!   * Уровень 1 — таблица ATCommandProcess @0x30149244: 11 пар
//!     «строка с \r» → mode (точное сравнение, strnicmp + длина);
//!     нет совпадения → mode 2 (данные DLOAD);
//!   * Уровень 2 — таблица atFindFunc @0x30154FB8: 16 записей
//!     {func, str, 0} — используется параллельным парсером;
//!   * ATCommandHandle (mode 0..0xA): jump-таблица @0x3007FA44:
//!     ATE0/ATE1 → "\r\nOK\r\n"; AT^DLOADVER? → "\r\n%s\r\n\r\nOK\r\n";
//!     AT^DLOADINFO? → dloadInfoATHandle; AT^AUTHORITYVER?/ID? →
//!     "\r\n%s\r\n\r\nOK\r\n" с геттерами; AT^DATAMODE → флаг data-mode
//!     (переход в двоичный DLOAD), ответ "\r\nOK\r\n"; AT^HWNATQRY? →
//!     "\r\n^HWNATQRY:%d\r\n\r\nOK\r\n"; AT^FLASHINFO? → формат-строка
//!     образа; AT+CMEE=0 → OK; AT^GODLOAD → флаг 0xA + OK;
//!     прочее → "\r\nERROR\r\n".

use crate::console;

/// Таблица уровня 1: строки, извлечённые из образа @0x3014957C..0x30149610.
/// (пары строка→mode, mode в коде ниже)
pub const AT_TABLE1: [(&[u8], u8); 11] = [
    (b"ATE0\r", 0x00),
    (b"ATE1\r", 0x01),
    (b"AT^DLOADVER?\r", 0x02),
    (b"AT^DLOADINFO?\r", 0x03),
    (b"AT^AUTHORITYVER?\r", 0x04),
    (b"AT^AUTHORITYID?\r", 0x05),
    (b"AT^DATAMODE\r", 0x06),
    (b"AT^HWNATQRY?\r", 0x07),
    (b"AT^FLASHINFO?\r", 0x08),
    (b"AT+CMEE=0\r", 0x09),
    (b"AT^GODLOAD\r", 0x0A),
];

/// Таблица уровня 2 (@0x30154FB8): строки + семантика обработчиков.
pub const AT_TABLE2: [&[u8]; 16] = [
    b"AT^DLOADVER?",
    b"AT^DLOADINFO?",
    b"AT^AUTHORITYVER?",
    b"AT^AUTHORITYID?",
    b"AT^DATAMODE",
    b"AT^BOOTROMVER?",
    b"AT^BOOTROMVER=?",
    b"AT^DLOADVER=?",
    b"AT^DLOADINFO=?",
    b"AT^AUTHORITYVER=?",
    b"AT^AUTHORITYID=?",
    b"AT^DATAMODE=?",
    b"ATE0",
    b"AT",
    b"AT^GETSDM",
    b"ATE1",
];

/// Строки-ответы (байт-в-байт из образа).
pub const RESP_OK: &[u8] = b"\r\nOK\r\n";
pub const RESP_ERROR: &[u8] = b"\r\nERROR\r\n";
pub const RESP_VER_FMT: &[u8] = b"\r\n%s\r\n\r\nOK\r\n";
pub const RESP_NATQRY_FMT: &[u8] = b"\r\n^HWNATQRY:%d\r\n\r\nOK\r\n";
pub const RESP_FLASHINFO_FMT: &[u8] =
    b"\r\n~~~~~~FLASH INFO~~~~~~:\r\n\r\nMMC BLOCK COUNT:%d,PAGE SIZE:%d,PAGE COUNT PER BLOCK:%d\r\n\r\nOK\r\n";
pub const RESP_AUTHID_PREFIX: &[u8] = b"\r\n";
pub const RESP_AUTHID_SUFFIX: &[u8] = b"\r\n\r\nOK\r\n";

/// Результат обработки строки ATCommandProcess.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtResult {
    /// Строка не является AT-командой — данные DLOAD (mode 2).
    DataMode,
    /// AT-команда обработана.
    Handled,
}

/// strnicmp-эквивалент (регистронезависимое сравнение до N символов).
fn strnicmp_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (x, y) in a.iter().zip(b.iter()) {
        if x.to_ascii_lowercase() != y.to_ascii_lowercase() {
            return false;
        }
    }
    true
}

/// Уровень 1: точный поиск по таблице (ATCommandProcess @0x3007FCE4).
pub fn process(line: &[u8]) -> AtResult {
    for (s, mode) in AT_TABLE1.iter() {
        if strnicmp_eq(s, line) {
            handle(*mode);
            return AtResult::Handled;
        }
    }
    AtResult::DataMode
}

/// Обработка режима (ATCommandHandle @0x3007F944, jump-таблица 11 веток).
fn handle(mode: u8) {
    match mode {
        0x00 | 0x01 => {
            // ATE0 / ATE1 → OK (эхо не влияет на двоичный протокол)
            send(RESP_OK);
        }
        0x02 => {
            // AT^DLOADVER? → "\r\n%s\r\n\r\nOK\r\n" (версия "2.0")
            let v = crate::version::DLOAD_VER;
            send_fmt_1(RESP_VER_FMT, v);
        }
        0x03 => {
            // AT^DLOADINFO? → dloadInfoATHandle: полная строка инфо
            send_dload_info();
        }
        0x04 => {
            // AT^AUTHORITYVER? → "1000"
            send_fmt_1(RESP_VER_FMT, crate::version::AUTHORITY_VER);
        }
        0x05 => {
            // AT^AUTHORITYID? → "\r\n" + ID + "\r\n\r\nOK\r\n"
            let mut id = [0u8; 24];
            let n = crate::version::authority_id(&mut id);
            send(RESP_AUTHID_PREFIX);
            send(&id[..n]);
            send(RESP_AUTHID_SUFFIX);
        }
        0x06 => {
            // AT^DATAMODE: флаг data-mode, ответ OK — терминатор AT-диалога
            crate::dload::set_data_mode(true);
            send(RESP_OK);
        }
        0x07 => {
            // AT^HWNATQRY? → "^HWNATQRY:%d" — сетевой режим
            send_natqry(0);
        }
        0x08 => {
            // AT^FLASHINFO? → геометрия MMC/NAND
            send_flashinfo(0x8000, crate::nand::PAGE_SIZE, crate::nand::PAGES_PER_BLOCK);
        }
        0x09 => {
            // AT+CMEE=0 → OK
            send(RESP_OK);
        }
        0x0A => {
            // AT^GODLOAD: немедленный уход в DLOAD (флаг 0xA), ответ OK
            crate::dload::set_data_mode(true);
            crate::dload::set_go_dload();
            send(RESP_OK);
        }
        _ => {
            send(RESP_ERROR);
        }
    }
}

/// Сборка AT^DLOADINFO? (dloadInfoATHandle → BSP_DLOAD_GetDloadInfo):
/// продукт, версия ПО, ISO-версия, midway-restart.
fn send_dload_info() {
    let p = crate::product::current();
    send(RESP_AUTHID_PREFIX); // "\r\n"
    send(p.product_name);
    send(b" ");
    send(crate::version::software_ver());
    send(b" ");
    send(p.dload_id);
    send(b" ");
    send(p.hw_ver);
    send(RESP_AUTHID_SUFFIX); // "\r\n\r\nOK\r\n"
}

/// Форматная подстановка "%s" (единственный формат, реально используемый
/// AT-геттерами загрузчика).
fn send_fmt_1(fmt: &[u8], arg: &[u8]) {
    let mut out = [0u8; 160];
    let mut o = 0usize;
    let mut i = 0usize;
    while i < fmt.len() && o + 1 < out.len() {
        if fmt[i] == b'%' && i + 1 < fmt.len() && fmt[i + 1] == b's' {
            for &b in arg {
                if o < out.len() {
                    out[o] = b;
                    o += 1;
                }
            }
            i += 2;
        } else {
            out[o] = fmt[i];
            o += 1;
            i += 1;
        }
    }
    send(&out[..o]);
}

/// Формат "%d" для ^HWNATQRY.
fn send_natqry(v: u32) {
    let mut out = [0u8; 48];
    let mut o = 0usize;
    let mut i = 0usize;
    while i < RESP_NATQRY_FMT.len() && o < out.len() {
        if RESP_NATQRY_FMT[i] == b'%' && i + 1 < RESP_NATQRY_FMT.len() && RESP_NATQRY_FMT[i + 1] == b'd' {
            let mut dec = [0u8; 10];
            let mut n = 0;
            let mut x = v;
            loop {
                dec[n] = b'0' + (x % 10) as u8;
                x /= 10;
                n += 1;
                if x == 0 {
                    break;
                }
            }
            while n > 0 && o < out.len() {
                n -= 1;
                out[o] = dec[n];
                o += 1;
            }
            i += 2;
        } else {
            out[o] = RESP_NATQRY_FMT[i];
            o += 1;
            i += 1;
        }
    }
    send(&out[..o]);
}

/// Формат FLASHINFO (3 x %d).
fn send_flashinfo(a: u32, b: u32, c: u32) {
    let mut out = [0u8; 160];
    let mut o = 0usize;
    let mut i = 0usize;
    let args = [a, b, c];
    let mut arg_i = 0usize;
    while i < RESP_FLASHINFO_FMT.len() && o < out.len() {
        if RESP_FLASHINFO_FMT[i] == b'%'
            && i + 1 < RESP_FLASHINFO_FMT.len()
            && RESP_FLASHINFO_FMT[i + 1] == b'd'
        {
            let mut dec = [0u8; 10];
            let mut n = 0;
            let mut x = args[arg_i.min(2)];
            arg_i += 1;
            loop {
                dec[n] = b'0' + (x % 10) as u8;
                x /= 10;
                n += 1;
                if x == 0 {
                    break;
                }
            }
            while n > 0 && o < out.len() {
                n -= 1;
                out[o] = dec[n];
                o += 1;
            }
            i += 2;
        } else {
            out[o] = RESP_FLASHINFO_FMT[i];
            o += 1;
            i += 1;
        }
    }
    send(&out[..o]);
}

/// Отправка ответа в USB (через DLOAD-канал: dloadUSBSend → BSP_DLOAD_SendData).
fn send(bytes: &[u8]) {
    crate::usb::dload_send(bytes);
}

/// Точка входа USB-RX колбэка (dloadUSBCallback @0x30081C80):
/// сначала ATCommandProcess; его результат 2 = данные DLOAD.
pub fn usb_rx(bytes: &[u8]) {
    if process(bytes) == AtResult::DataMode {
        // каждый байт через HDLC-декапсулятор (Om_HdlcDecap)
        crate::dload::feed(bytes);
    }
}
