//! wdt.rs — сторожевой таймер SP805 @0x90001000.
//!
//! ОРИГИНАЛ (docs/02 §2.2/§3.5, все адреса подтверждены литералами):
//!   0x90001000 WDT_LOAD   : 0xF0000 (30 c при 32768 Гц) / param / 10 (ребут)
//!   0x90001004 WDT_VALUE
//!   0x90001008 WDT_CONTROL: 3 = ENABLE|RESETEN (Enable/Resume), 0 (Disable/Stop)
//!   0x9000100C WDT_INTCLR : 0 — «кормление» (BSP_WDT_HardwareFeed)
//!   0x90001C00 WDT_LOCK   : магия разблокировки 0x1ACCE551
//! Клок-гейт: 0x9000000C (enable, бит31), 0x90000010 (disable),
//! 0x90000014 (status). Регистр 0x90000408 = 0x69200001 при BSP_WDT_Enable.

use crate::mmio::*;

/// База SP805.
pub const WDT_BASE: usize = 0x9000_1000;
/// Регистр LOCK (смещение 0xC00).
pub const WDT_LOCK_REG: usize = 0x9000_1C00;
/// Магия разблокировки SP805.
pub const WDT_UNLOCK: u32 = 0x1ACCE_551;
/// Аппаратный WDT унаследован от bootrom: 0xF0000 = 30 с при 32768 Гц.
pub const WDT_LOAD_BOOTROM: u32 = 0xF_0000;

#[inline]
unsafe fn lock() {
    write32(WDT_LOCK_REG, 0); // любое значение != 0x1ACCE551 запирает
}

#[inline]
unsafe fn unlock() {
    write32(WDT_LOCK_REG, WDT_UNLOCK);
}

/// Тактирование WDT: включить (PWRCTRL_WDT_SoftClkEn @0x30007184:
/// `*0x9000000C = 0x80000000`).
pub fn clock_enable() {
    unsafe { write32(0x9000_000C, 0x8000_0000) };
}

/// Тактирование WDT: выключить (PWRCTRL_WDT_SoftClkDis @0x30007174).
pub fn clock_disable() {
    unsafe { write32(0x9000_0010, 0x8000_0000) };
}

/// Включить WDT на 30 секунд (BSP_WDT_Enable @0x30007624):
/// клок-гейт, `*0x90000408 = 0x69200001`, UNLOCK, LOAD=0xF0000, CONTROL=3, LOCK.
pub fn enable() {
    unsafe {
        clock_enable();
        write32(0x9000_0408, 0x6920_0001); // сигнатура чипа HI6920 (docs/02 §2.1)
        unlock();
        write32(WDT_BASE + 0x000, WDT_LOAD_BOOTROM);
        write32(WDT_BASE + 0x008, 3); // ENABLE | RESETEN
        lock();
    }
}

/// Выключить WDT (BSP_WDT_Disable @0x300076D8: CONTROL=0, клок-гейт off).
pub fn disable() {
    unsafe {
        unlock();
        write32(WDT_BASE + 0x008, 0);
        lock();
        clock_disable();
    }
}

/// Остановить WDT (BSP_WDT_Stop @0x3000797C: CONTROL=0 — без клок-гейта).
/// Вызывается из BSP_DRV_BOOTROM_Init вокруг долгих флеш-операций.
pub fn stop() {
    unsafe {
        unlock();
        write32(WDT_BASE + 0x008, 0);
        lock();
    }
}

/// Возобновить WDT (BSP_WDT_Resume @0x300079A4: CONTROL=3).
pub fn resume() {
    unsafe {
        unlock();
        write32(WDT_BASE + 0x008, 3);
        lock();
    }
}

/// «Кормление» напрямую (BSP_WDT_HardwareFeed @0x300071B4: INTCLR = 0).
pub fn feed_force() {
    unsafe {
        unlock();
        write32(WDT_BASE + 0x00C, 0);
        lock();
    }
}

/// Немедленный ребут через WDT (BSP_WDT_reboot_direct @0x30007910):
/// LOAD=10, CONTROL=3, вечный цикл — сброс по истечении.
pub fn reboot_direct() -> ! {
    unsafe {
        clock_enable();
        unlock();
        write32(WDT_BASE + 0x008, 3);
        write32(WDT_BASE + 0x000, 10);
        lock();
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Чтение текущего значения счётчика (BSP_WDT_TimeoutGet использует VALUE).
pub fn value_get() -> u32 {
    unsafe { read32(WDT_BASE + 0x004) }
}
