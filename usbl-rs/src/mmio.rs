//! mmio.rs — примитивы доступа к регистрам MMIO (volatile).
//!
//! Как в оригинале: все обращения к периферии Balong — прямые чтения/записи слов
//! по абсолютным адресам (см. docs/02_BSP_DRIVERS.md репозитория ANUS).

#![allow(dead_code)]

/// Чтение 32-битного слова по абсолютному адресу.
#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

/// Запись 32-битного слова по абсолютному адресу.
#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val)
}

/// Чтение 8-битного байта.
#[inline(always)]
pub unsafe fn read8(addr: usize) -> u8 {
    core::ptr::read_volatile(addr as *const u8)
}

/// Запись 8-битного байта.
#[inline(always)]
pub unsafe fn write8(addr: usize, val: u8) {
    core::ptr::write_volatile(addr as *mut u8, val)
}

/// Read-modify-write: прочитать, наложить маску, записать.
/// Эквивалент оригинальных паттернов `*REG = (*REG & ~clr) | set`.
#[inline(always)]
pub unsafe fn rmw32(addr: usize, clear: u32, set: u32) {
    let v = read32(addr);
    write32(addr, (v & !clear) | set);
}

/// Ожидание установки всех битов маски (поллинг готовности).
/// Эквивалент оригинальных циклов `ldr/and/cmp/bne`.
#[inline(always)]
pub unsafe fn wait_bits_set(addr: usize, mask: u32) {
    while read32(addr) & mask != mask {
        core::hint::spin_loop();
    }
}

/// Ожидание сброса всех битов маски.
#[inline(always)]
pub unsafe fn wait_bits_clear(addr: usize, mask: u32) {
    while read32(addr) & mask != 0 {
        core::hint::spin_loop();
    }
}

/// Программная задержка в «холостых» итерациях.
/// Тот же приём, что в оригинальном стабе (счётчик 0x4E20, docs/01 §8.1)
/// и в ABB_Init (задержки 0x19BFCC / 0x83D6).
#[inline(always)]
pub fn delay_loop(iterations: u32) {
    let mut n = iterations;
    while n != 0 {
        core::hint::spin_loop();
        n -= 1;
    }
}

/// Задержка в миллисекундах (примерно, калибровка консервативная:
/// ядро Cortex-A9 @ ~500 МГц, ~5 тактов на итерацию delay_loop).
pub fn delay_ms(ms: u32) {
    delay_loop(ms.saturating_mul(700));
}
