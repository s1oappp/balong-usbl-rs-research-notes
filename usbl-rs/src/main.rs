//! usbl-rs — полная Rust-реконструкция USB-загрузчика (USBL, burn-режим)
//! Huawei E3272s-153 (Balong V7R1, HI6920, ARM Cortex-A9).
//!
//! ИСТОЧНИК: github.com/s1oappp/ANUS (E3272s-153_USBL) — декомпиляция и
//! документация usblsafe-327x.bin (VxWorks 6.8, 21.431.99.07.00,
//! 1 754 444 байта, SHA-256 8e093982…6805).
//!
//! МОДЕЛЬ: no_std, голое железо (VxWorks 6.8 заменён детерминированным
//! главным циклом + IRQ; см. модули). Порядок запуска повторяет
//! оригинальный usrRoot → usrAppInit (docs/01 §6):
//!   1. консоль UART (sysSerialInit: PL011 115200);
//!   2. баннер (как «as Boot ROM» / логи VxWorks);
//!   3. nand_init + слово-ловушка 0x2ffffffc == 0x98765432
//!      (иначе nand_erase_all — то же поведение, что в оригинале);
//!   4. WDT-операции вокруг флеша (Feed/Stop — как в оригинале);
//!   5. dloadInit (HDLC/DCB/буферы);
//!   6. USB_UnReset + стек (tUSBTask → jusb_entry);
//!   7. главный цикл: poll USB (и опрос UART-консоли).
//!
//! СОВМЕСТИМОСТЬ (выбранная трактовка «интерфейсы + данные»):
//!   * формат образа байт-в-байт: тот же заголовок 84 байта (include_bytes!
//!     из оригинала), тот же размер 1 754 444, те же адреса/входы
//!     (стаб @0x2FFE4000, тело @0x30004000);
//!   * таблицы и строки, встроенные из образа байт-в-байт: FCS-16,
//!     ptable, huaweiProduct, AT-таблицы, форматы ответов, статические
//!     HDLC-кадры;
//!   * протоколы: PPP-FCS HDLC, AT-команды, DLOAD-пакеты 0x41/42/43,
//!     ответы 'E'/'F'/'K', защита NAND-словом.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]
// API-поверхность повторяет оригинал (все BSP_*-эквиваленты реализованы,
// даже если главный цикл использует не все) — dead_code здесь не дефект:
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_mut)]
#![allow(unused_unsafe)]
#![allow(unused_parens)]

mod at;
mod console;
mod dload;
mod hdlc;
mod mmio;
mod nand;
mod product;
mod ptable;
mod safety;
mod start;
mod stub;
mod usb;
mod version;
mod vic;
mod wdt;

/// Заголовок bootrom (84 байта), извлечённый из оригинального
/// usblsafe-327x.bin байт-в-байт. Паковщик (tools/pack.py) кладёт его
/// в начало .bin без изменений — поля +0x28 (размер стаба 0x36C),
/// +0x2C (вход 0x2FFE4000), +0x38 (размер тела 0x1AC18C),
/// +0x3C (адрес 0x30004000), +0x40 (смещение тела 0x3C0) остаются
/// валидными, потому что паковщик добивает секции до исходных размеров.
#[link_section = ".origdata"]
#[used]
static ORIGINAL_HEADER: [u8; 84] = *include_bytes!("../data/header.bin");

/// Главный цикл (эквивалент usrRoot → usrAppInit + задачи tUSBTask/
/// tDloadTask/tIdletask в одном детерминированном цикле).
#[no_mangle]
pub extern "C" fn rust_main() {
    // ---- usrSerialInit: консоль ----
    console::init();
    console::putln("");
    console::putln("USBL-RS (Rust) — Balong V7R1 / HI6920");
    console::puts("Сборка для E3272s-153, продукт ");
    console::write_bytes(product::current().product_name);
    console::puts(", DLOAD_ID ");
    console::write_bytes(product::current().dload_id);
    console::putln("");
    console::puts("Замена usblsafe-327x.bin (VxWorks 6.8, 21.431.99.07.00),");
    console::putln("интерфейсная совместимость + данные из образа");

    // ---- контроллер прерываний (sysHwInit2: intLibInit + armGicCtlInit) ----
    vic::init();
    unsafe { vic::cpu_irq_enable() };

    // ---- nand_init (usrAppInit:934) ----
    let nand_ok = nand::init();
    if nand_ok {
        console::putln("NAND: контроллер 0x900A4000, 256 МБ, ptable @0x1E000");
    } else {
        console::putln("NAND: контроллер не отвечает (магия 0x19840815)");
    }

    // ---- слово-ловушка персистентной SRAM (usrAppInit:938-939) ----
    // _DAT_2ffffffc != 0x98765432 → nand_erase_all()
    safety::check_and_protect();

    // ---- WDT: кормление/остановка вокруг инициализации (usrAppInit:941-948) ----
    wdt::feed_force();
    wdt::stop();

    // ---- ptable в RAM (BSP_DRV_BOOTROM_Init) ----
    ptable::init();
    console::puts("ptable: ");
    console::put_u32_dec(ptable::count() as u32);
    console::putln(" разделов загружено");

    // ---- dloadInit (usrAppInit:960) ----
    dload::init();

    // ---- USB_UnReset + стек (usrAppInit:961, tUSBTask) ----
    usb::init();

    // ---- WDT: возобновление после инициализации (usrAppInit:947) ----
    wdt::resume();
    wdt::feed_force();

    console::putln("USBL: готов, ожидаю AT/DLOAD по USB (PID 0x1C05)");

    // ---- главный цикл: tUSBTask + tDloadTask + tIdletask ----
    let mut iterations: u32 = 0;
    loop {
        // USB-события (в оригинале — ISR os_generic_isr; здесь — ISR+poll)
        usb::poll();

        // консоль UART: переваривать ввод (shell-канал оригинала)
        while console::rx_ready() {
            let _ = console::getc();
        }

        // периодическое кормление WDT (WDT_LowTask: раз в 500 тиков)
        iterations = iterations.wrapping_add(1);
        if iterations % 500_000 == 0 {
            wdt::feed_force();
        }

        core::hint::spin_loop();
    }
}

/// Паника: зависание с диагностикой (загрузчик не имеет деградировавшего
/// состояния — совместимость с panic=abort оригинального VxWorks-стека).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    console::putln("USBL: ПАНИКА");
    if let Some(loc) = info.location() {
        console::puts("  @ ");
        console::puts(loc.file());
        console::putc(b':');
        console::put_u32_dec(loc.line() as u32);
        console::putln("");
    }
    loop {
        core::hint::spin_loop();
    }
}
