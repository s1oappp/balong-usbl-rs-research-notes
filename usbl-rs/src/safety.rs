//! safety.rs — слово-ловушка персистентной SRAM (0x2ffffffc).
//!
//! ОРИГИНАЛ (decompiled_all.c:938-939, docs/01 §6, docs/11 §3.3):
//! при старте usrAppInit сравнивает слово персистентной SRAM @0x2ffffffc
//! с эталоном DAT_3000511c = **0x98765432**; при несовпадении вызывается
//! nand_erase_all() — ПОЛНОЕ СТИРАНИЕ NAND (анти-brick против повреждённой
//! ФС/NV). Значение слова выставляет загрузочная среда (RM/midway-restart);
//! самодельная перезагрузка в обход штатного пути может дать полный erase.
//!
//! ЗДЕСЬ: та же проверка в том же месте загрузочного пути; путь стирания
//! выводит диагностику на консоль перед erase (совместимость поведения
//! сохранена, добавлен только лог).

/// Адрес контрольного слова в персистентной SRAM.
pub const CHECK_WORD_ADDR: usize = 0x2FFF_FFFC;
/// Эталонное значение (DAT_3000511c из образа).
pub const CHECK_WORD_MAGIC: u32 = 0x9876_5432;

/// Проверка слова-ловушки. Возвращает true, если NAND можно НЕ стирать.
/// При false вызывается nand_erase_all() — как в оригинале.
pub fn check_and_protect() -> bool {
    let word = unsafe { crate::mmio::read32(CHECK_WORD_ADDR) };
    if word == CHECK_WORD_MAGIC {
        true
    } else {
        crate::console::puts("\r\nUSBL: SRAM-маркер != 0x98765432 — ");
        crate::console::put_u32_hex(word, 8);
        crate::console::putln(", стираю NAND (как в оригинале)");
        crate::nand::erase_all();
        // после стирания выставить маркер (midway-restart контракт)
        unsafe { crate::mmio::write32(CHECK_WORD_ADDR, CHECK_WORD_MAGIC) };
        false
    }
}

/// Выставить маркер (используется перед мягкой перезагрузкой — контракт
/// «загрузчик ↔ среда», docs/10 §1.6).
pub fn set_marker() {
    unsafe { crate::mmio::write32(CHECK_WORD_ADDR, CHECK_WORD_MAGIC) };
}
