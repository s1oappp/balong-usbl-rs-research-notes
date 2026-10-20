//! vic.rs — контроллер прерываний HI6920 (база 0x08000000, 160 линий).
//!
//! РЕКОНСТРУКЦИЯ (armGicCtlInit @0x30007EF4, FUN_30007c30, FUN_30007f84,
//! FUN_3000805c, armGicHwEnable @0x30008354 — карта регистров извлечена из
//! образа, semantics GIC-подобные):
//!   +0x0100 — глобальное включение (запись 1)
//!   +0x0104 — конфигурация (запись 0xFF)
//!   +0x0108 — конфигурация (запись 2)
//!   +0x010C — IAR: чтение номера прерывания (маска 0x3FF; 0x3FF = spurious)
//!   +0x0110 — EOI: запись номера прерывания (конец обработки)
//!   +0x1000 — включение CPU-интерфейса (запись 1)
//!   +0x1100 + 4*(n/32) — set-enable линии n (бит n%32)
//!   +0x1180 + 4*(n/32) — clear-enable
//!   +0x1280 + 4*(n/32) — clear-pending
//!   +0x1400 + 4*n       — целевой CPU
//!   +0x1800 + 4*n       — приоритет
//!   +0x1C00 + 4*(n/16)  — конфигурация edge/level (0x55555555)
//! Инициализация (FUN_30007c30): все линии 32..159 выключены, приоритеты и
//! цели из таблиц, фильтр 0x1C10 |= 0xA0, 0x1C24 |= 0x800.
//!
//! Используемые линии (docs/08 §5): UART 102, SD/MMC 108, PMU 127,
//! WDT 84/37, GPIO 112–117, системные 90/134.

use crate::mmio::*;

/// База контроллера прерываний (armGicCtlInit: `*0x3017D4A4 = 0x08000000`).
pub const VIC_BASE: usize = 0x0800_0000;
/// Число линий (intLibInit(0xa0,0xa0,1) → 160).
pub const VIC_NUM_LINES: u32 = 160;
/// Маска поля номера в IAR.
const VIC_IAR_MASK: u32 = 0x3FF;

/// Обработчик прерывания: аргумент — номер линии.
pub type IrqHandler = fn(u32);

const MAX_HANDLERS: usize = 160;

static mut HANDLERS: [Option<IrqHandler>; MAX_HANDLERS] = [None; MAX_HANDLERS];

/// Регистрация обработчика (аналог intConnect/intArchConnect: таблица
/// {handler, arg} в RAM).
pub fn connect(line: u32, handler: IrqHandler) {
    if (line as usize) < MAX_HANDLERS {
        unsafe {
            HANDLERS[line as usize] = Some(handler);
        }
    }
}

/// Включить линию (armGicHwEnable: set-enable по маске бита).
pub fn enable_line(line: u32) {
    if line >= VIC_NUM_LINES || line < 0x20 {
        return; // SGI/PPI не включаются через распределитель
    }
    unsafe {
        let reg = VIC_BASE + 0x1100 + ((line / 32) as usize) * 4;
        rmw32(reg, 0, 1 << (line % 32));
    }
}

/// Выключить линию (clear-enable).
pub fn disable_line(line: u32) {
    if line >= VIC_NUM_LINES {
        return;
    }
    unsafe {
        let reg = VIC_BASE + 0x1180 + ((line / 32) as usize) * 4;
        write32(reg, 1 << (line % 32));
    }
}

/// Полная инициализация контроллера (как FUN_30007c30).
pub fn init() {
    unsafe {
        // маска/фильтры распределителя
        write32(VIC_BASE + 0x1280, 0xF800_FFFF);
        // приоритеты/цели для линий 0..31 (значения FUN_30007c30)
        for off in (0..0x10).step_by(4) {
            write32(VIC_BASE + 0x1400 + off, 0xC0C0_C0C0);
        }
        for n in (0..16).step_by(4) {
            write32(VIC_BASE + 0x1C00 + n, 0xAAAA_AAAA);
        }
        for off in (0x10..0x20).step_by(4) {
            write32(VIC_BASE + 0x1400 + off, 0xF0F0_F0F0);
        }
        // выключить/очистить линии 32..159
        let n_words = (VIC_NUM_LINES / 32) as usize; // 5 слов: 32..159
        for w in 1..=n_words {
            write32(VIC_BASE + 0x1180 + w * 4, 0xFFFF_FFFF);
            write32(VIC_BASE + 0x1280 + w * 4, 0xFFFF_FFFF);
        }
        // конфигурация edge/level
        for n in (2..10usize) {
            write32(VIC_BASE + 0x1C00 + n * 4, 0x5555_5555);
        }
        // глобальные биты
        rmw32(VIC_BASE + 0x1C10, 0, 0xA0);
        rmw32(VIC_BASE + 0x1C24, 0, 0x800);
        // включить распределитель и CPU-интерфейс
        write32(VIC_BASE + 0x104, 0xFF);
        write32(VIC_BASE + 0x108, 2);
        write32(VIC_BASE + 0x100, 1);
        write32(VIC_BASE + 0x1000, 1);
    }
}

/// Диспетчер IRQ (вызывается из asm irq_handler; семантика FUN_30007f84 +
/// FUN_3000805c: чтение IAR, обработка, запись EOI).
#[no_mangle]
pub extern "C" fn irq_dispatch() {
    unsafe {
        let iar = read32(VIC_BASE + 0x10C);
        let line = iar & VIC_IAR_MASK;
        if line == VIC_IAR_MASK {
            return; // spurious
        }
        let n = if line < 0x10 { line + VIC_NUM_LINES } else { line };
        if (n as usize) < MAX_HANDLERS {
            if let Some(h) = HANDLERS[n as usize] {
                h(n);
            }
        }
        // EOI (FUN_3000805c: запись номера в +0x110)
        write32(VIC_BASE + 0x110, n);
    }
}

/// Глобальное разрешение IRQ в CPSR (аналог intUnlock(0)).
pub unsafe fn cpu_irq_enable() {
    core::arch::asm!(
        "mrs r0, cpsr",
        "bic r0, r0, #0x80",
        "msr cpsr_c, r0",
        out("r0") _
    );
}

/// Глобальный запрет IRQ в CPSR (аналог intLock).
pub unsafe fn cpu_irq_disable() {
    core::arch::asm!(
        "mrs r0, cpsr",
        "orr r0, r0, #0x80",
        "msr cpsr_c, r0",
        out("r0") _
    );
}
