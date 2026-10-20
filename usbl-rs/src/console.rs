//! console.rs — консольный UART: PL011 (ARM PrimeCell) @0x90007000.
//!
//! ОРИГИНАЛ: docs/08 §5 — UART 0x90007000, PL011-совместимый, 1 канал,
//! baud 115200, входной клок 48 МГц (0x02DC6C00), IRQ 102; stdin/stdout
//! VxWorks через ttyDrv (usrSerialInit). Здесь: прямой опросный ввод/вывод
//! (polling) — для загрузчика прерывания RX не обязательны (USB — основной
//! канал прошивки, UART — диагностика).

use crate::mmio::*;

/// База UART0.
pub const UART0_BASE: usize = 0x9000_7000;
/// Входная частота UART (48 МГц, docs/08).
pub const UART_CLK: u32 = 48_000_000;
/// Скорость консоли.
pub const UART_BAUD: u32 = 115_200;

// Регистры PL011 (смещения от базы).
const UART_DR: usize = 0x000; // данные (RX/TX)
const UART_FR: usize = 0x018; // флаги
const UART_IBRD: usize = 0x024; // целая часть делителя
const UART_FBRD: usize = 0x028; // дробная часть делителя
const UART_LCRH: usize = 0x02C; // управление линией
const UART_CR: usize = 0x030; // управление UART
const UART_ICR: usize = 0x044; // сброс прерываний

const FR_TXFF: u32 = 1 << 5; // FIFO передачи полон
const FR_RXFE: u32 = 1 << 4; // FIFO приёма пуст
const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;

/// Инициализация: 115200 8N1, FIFO включён (как primeCellSioDevInit оригинала).
pub fn init() {
    unsafe {
        // Делители: baud = clk / (16 * (IBRD + FBRD/64))
        let divisor = (UART_CLK * 4 + UART_BAUD * 2) / UART_BAUD; // (IBRD + FBRD/64)*4, округление
        let ibrd = divisor / 4;
        let fbrd = divisor % 4; // уже в четвертях: FBRD = x64 от четверти → *16
        write32(UART0_BASE + UART_CR, 0); // выключить на время настройки
        write32(UART0_BASE + UART_ICR, 0x7FF); // сбросить все прерывания
        write32(UART0_BASE + UART_IBRD, ibrd);
        write32(UART0_BASE + UART_FBRD, fbrd * 16);
        // 8N1, FIFO: WLEN=8 (bits6-5=11), FEN=bit4
        write32(UART0_BASE + UART_LCRH, (3 << 5) | (1 << 4));
        // Включить UART, TX, RX
        write32(UART0_BASE + UART_CR, CR_UARTEN | CR_TXE | CR_RXE);
    }
}

/// Послать один байт (с ожиданием готовности FIFO).
pub fn putc(c: u8) {
    unsafe {
        while read32(UART0_BASE + UART_FR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        write32(UART0_BASE + UART_DR, c as u32);
    }
}

/// Послать строку байт.
pub fn write_bytes(bytes: &[u8]) {
    for &b in bytes {
        putc(b);
    }
}

/// Послать C-строку (без завершающего NUL).
pub fn puts(s: &str) {
    write_bytes(s.as_bytes());
}

/// Строка с CRLF (в оригинале VxWorks logMsg сам добавляет \n).
pub fn putln(s: &str) {
    puts(s);
    putc(b'\r');
    putc(b'\n');
}

/// Есть ли принятый байт?
pub fn rx_ready() -> bool {
    unsafe { read32(UART0_BASE + UART_FR) & FR_RXFE == 0 }
}

/// Принять байт (блокирующе).
pub fn getc() -> u8 {
    unsafe {
        while read32(UART0_BASE + UART_FR) & FR_RXFE != 0 {
            core::hint::spin_loop();
        }
        (read32(UART0_BASE + UART_DR) & 0xFF) as u8
    }
}

/// Простой форматированный вывод чисел (замена printf оригинала для диагностики).
pub fn put_u32_hex(mut v: u32, digits: usize) {
    puts("0x");
    let mut buf = [0u8; 8];
    for i in (0..digits).rev() {
        let nib = (v & 0xF) as u8;
        buf[i] = if nib < 10 { b'0' + nib } else { b'A' + nib - 10 };
        v >>= 4;
    }
    write_bytes(&buf[..digits]);
}

/// Десятичный вывод u32.
pub fn put_u32_dec(mut v: u32) {
    let mut buf = [0u8; 10];
    let mut i = 0;
    loop {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
        if v == 0 {
            break;
        }
    }
    let mut out = [0u8; 10];
    for k in 0..i {
        out[k] = buf[i - 1 - k];
    }
    write_bytes(&out[..i]);
}
