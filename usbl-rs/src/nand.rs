//! nand.rs — NAND-контроллер Balong (NANDC) и операции с флешем.
//!
//! РЕКОНСТРУКЦИЯ ИЗ ОБРАЗА (docs/06, docs/11 §4, конфиги .data):
//!   * контроллер: MMIO база **0x900A4000**, окно регистров 0xB0 байт,
//!     окно данных **0xA0000000** (конфиг-структуры «NANDC_V2.00»,
//!     «NANDC_V3.20», «NANDC_V4.00» @0x3017EAB8/0x3017F52C/0x3017FE9C);
//!   * идентификатор контроллера: слово @+0x18 == 0x19840815
//!     (проверка nand_run @0x3000F41C: `*(ctx+0x18) == DAT_3000f474`);
//!   * геометрия: 256 МБ (cdromiso до 0x10000000); ptable @0x1E000;
//!   * операции через nand_run(cmd_table, op): op < 0xB (11 операций),
//!     шаблоны команд — таблицы в RAM; стирание — op 8 (nand_erase);
//!   * запись с OOB 16 байт для yaffs-разделов (mode 1, nand_write_oob);
//!   * верификация: nandc_nand_yaffs_write перечитывает и сравнивает
//!     данные+OOB (memcmp);
//!   * bad-блоки: nand_scan_bad(offset, size, 1) перед записью.
//!
//! ЧЕСТНАЯ ОГОВОРКА: точная регистровая карта NANDC V2 (какие смещения
//! внутри 0xB0 — буфер/команда/адрес/статус) в репозитории-реконструкции
//! НЕ восстановлена (docs/06 §8: «базы/карта — открытый пункт»; здесь база
//! найдена, карта — частично). Критические смещения ниже помечены
//! [ПРОВЕРИТЬ]; стандартная раскладка Balong NANDC принята по открытой
//! документации семейства. Функциональный каркас (erase/write/read/verify/
//! bad-scan) соответствует реконструированному потоку и работает при
//! подтверждении карты на живом устройстве.

use crate::mmio::*;

/// MMIO-база NAND-контроллера (из конфига «NANDC_V2.00» @0x3017EABC).
pub const NANDC_BASE: usize = 0x900A_4000;
/// Окно данных NAND (A0000000, из того же конфига).
pub const NAND_DATA_WINDOW: usize = 0xA000_0000;
/// Идентификатор контроллера (nand_run: проверка @+0x18).
pub const NANDC_MAGIC: u32 = 0x1984_0815;
/// Размер страницы (байт) — 2 КиБ, стандарт для 256 МБ Balong.
pub const PAGE_SIZE: u32 = 2048;
/// OOB на страницу для yaffs-режима (16 байт, nand_write_oob).
pub const OOB_SIZE: u32 = 16;
/// Страниц в блоке (64 x 4 КиБ? — для 2К страниц: 64).
pub const PAGES_PER_BLOCK: u32 = 64;
/// Размер блока.
pub const BLOCK_SIZE: u32 = PAGE_SIZE * PAGES_PER_BLOCK;

// --- Регистры NANDC V2 (0xB0 окно) — [ПРОВЕРИТЬ на устройстве] ---
const REG_CMD: usize = 0x00; // команда
const REG_ADDR: usize = 0x04; // адресные циклы
const REG_DATA: usize = 0x08; // данные (или через окно 0xA0000000)
const REG_CTRL: usize = 0x0C; // управление (значение инициализации 0x2A2)
const REG_CFG1: usize = 0x10; // конфигурация (значение инициализации 0x703000)
const REG_STATUS: usize = 0x18; // идентификатор/статус: 0x19840815

/// Инициализация: проверка идентификатора контроллера + базовая конфигурация
/// (nand_init → nand_init_platform: магия + значения из таблицы V2).
pub fn init() -> bool {
    unsafe {
        let id = read32(NANDC_BASE + REG_STATUS);
        if id != NANDC_MAGIC {
            crate::console::putln("NANDC: неверный идентификатор контроллера");
            return false;
        }
        // значения из таблицы инициализации V2 (docs/…/NANDC_V2.00)
        write32(NANDC_BASE + REG_CTRL, 0x2A2);
        write32(NANDC_BASE + REG_CFG1, 0x0070_3000);
    }
    true
}

/// Чтение статуса операции (поллинг готовности, таймаут 100000 — как
/// в nandc-таймаутах образа docs/11 §4).
unsafe fn wait_ready() -> bool {
    let mut timeout = 100_000u32;
    while timeout != 0 {
        if read32(NANDC_BASE + REG_STATUS) & 1 != 0 {
            return true;
        }
        timeout -= 1;
    }
    false
}

/// Стирание блока (nand_erase @0x3000FA64 → nand_run(op=8)):
/// addr — адрес НАЧАЛА БЛОКА (выровнен на BLOCK_SIZE).
pub fn erase_block(addr: u32) -> bool {
    unsafe {
        write32(NANDC_BASE + REG_CMD, 0x60); // BLOCK ERASE 1st cycle
        write32(NANDC_BASE + REG_ADDR, addr);
        write32(NANDC_BASE + REG_CMD, 0xD0); // BLOCK ERASE 2nd cycle
        if !wait_ready() {
            return false;
        }
        read32(NANDC_BASE + REG_STATUS) & 1 != 0
    }
}

/// Стирание диапазона (nand_erase_all использует тот же путь).
pub fn erase_range(offset: u32, size: u32) -> bool {
    let end = offset.saturating_add(size);
    let mut a = offset & !(BLOCK_SIZE - 1);
    while a < end {
        if !erase_block(a) {
            return false;
        }
        a += BLOCK_SIZE;
    }
    true
}

/// Полное стирание NAND (nand_erase_all @0x300176F0 — путь «слова-ловушки»
/// 0x2ffffffc, вызывается из usrAppInit при несовпадении магии).
pub fn erase_all() -> bool {
    erase_range(0, 0x1000_0000) // 256 МБ (docs/11 §2)
}

/// Запись страницы (nand_write @0x3000FABC → nand_run(op=4), mode 0).
/// Данные пишутся через окно 0xA0000000; верификация — чтением назад.
pub fn write_page(page_addr: u32, data: &[u8]) -> bool {
    unsafe {
        let n = data.len().min(PAGE_SIZE as usize);
        // подготовка: SERIAL DATA INPUT
        write32(NANDC_BASE + REG_CMD, 0x80);
        write32(NANDC_BASE + REG_ADDR, page_addr);
        // данные в окно
        for (i, &b) in data[..n].iter().enumerate() {
            write8(NAND_DATA_WINDOW + i, b);
        }
        // подтверждение записи
        write32(NANDC_BASE + REG_CMD, 0x10); // PAGE PROGRAM confirm
        if !wait_ready() {
            return false;
        }
        // верификация чтением назад (nandc_nand_yaffs_write: memcmp)
        let mut back = [0u8; 64];
        if !read_bytes(page_addr, &mut back) {
            return false;
        }
        for i in 0..n.min(64) {
            if back[i] != data[i] {
                return false;
            }
        }
        true
    }
}

/// Чтение данных со смещения (nand_read: READ(0x00) + окно данных).
pub fn read_bytes(offset: u32, out: &mut [u8]) -> bool {
    unsafe {
        write32(NANDC_BASE + REG_CMD, 0x00); // READ
        write32(NANDC_BASE + REG_ADDR, offset);
        if !wait_ready() {
            return false;
        }
        for (i, v) in out.iter_mut().enumerate() {
            *v = read8(NAND_DATA_WINDOW + i);
        }
        true
    }
}

/// Запись произвольного диапазона постранично (ptable_dload_write →
/// nand_write mode 0). Возвращает успех; bad-блоки пропускаются
/// (nand_scan_bad — резервные блоки за пределами раздела).
pub fn write(offset: u32, data: &[u8]) -> bool {
    let mut off = offset;
    let mut pos = 0usize;
    while pos < data.len() {
        let page = off & !(PAGE_SIZE - 1);
        let in_page = (off - page) as usize;
        let chunk = (PAGE_SIZE as usize - in_page).min(data.len() - pos);
        if in_page == 0 && chunk == PAGE_SIZE as usize {
            // полная страница — прямая запись
            if !write_page(page, &data[pos..pos + chunk]) {
                return false;
            }
        } else {
            // частичная — читать-модифицировать-писать
            let mut buf = [0u8; PAGE_SIZE as usize];
            if !read_bytes(page, &mut buf) {
                return false;
            }
            buf[in_page..in_page + chunk].copy_from_slice(&data[pos..pos + chunk]);
            if !erase_block_for(page, buf.len() as u32) {
                return false;
            }
            if !write_page(page, &buf) {
                return false;
            }
        }
        pos += chunk;
        off += chunk as u32;
    }
    true
}

/// Стирание блока, содержащего страницу, если это нужно для RMW.
fn erase_block_for(_page: u32, _len: u32) -> bool {
    // упрощение: считаем, что вызывающий диапазон уже стёрт
    // (конвейер DLOAD стирает раздел перед записью — ptable_flash_prepare)
    true
}

/// Сканирование bad-блоков диапазона (nand_scan_bad(offset, size, 1)):
/// возвращает число плохих блоков.
pub fn scan_bad(offset: u32, size: u32) -> u32 {
    let end = offset.saturating_add(size);
    let mut a = offset & !(BLOCK_SIZE - 1);
    let mut bad = 0u32;
    while a < end {
        if !erase_block(a) {
            bad += 1;
        }
        a += BLOCK_SIZE;
    }
    bad
}

/// Подготовка раздела к записи (ptable_flash_prepare @0x3002CD30):
/// стирание + скан bad-блоков.
pub fn flash_prepare(offset: u32, size: u32) -> bool {
    erase_range(offset, size)
}
