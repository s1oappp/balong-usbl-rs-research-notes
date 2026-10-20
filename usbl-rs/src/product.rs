//! product.rs — таблица продуктов huaweiProduct (байт-в-байт из образа).
//!
//! ОРИГИНАЛ: записи 17 x 0x34 байт @0x3012FAD8..0x3012FE80 + строковый пул
//! 0x3012FE80..0x30130028 (docs/07 §1). Структура записи:
//!   +0x00 id (main<<8|sub), +0x04 productName, +0x08 namePlus,
//!   +0x0C dloadId, +0x10 hwVer, +0x14 innerName, +0x18 bands0,
//!   +0x1C bands1, +0x20 bands2 (указатели — VA оригинального образа).
//! Эта сборка — для E3272s-153 (id 0x301); при пустой NV грузится запись
//! по умолчанию 0xFFFF (MPWUDP).
//!
//! Встроенный ниже блоб — точные 1360 байт образа; указатели внутри
//! пересчитаны при парсинге относительно базы блоба (оригинальные VA
//! сохранены в комментарии для сверки).

/// Таблица продуктов + строковый пул, извлечённые из usblsafe-327x.bin
/// (VA 0x3012FAD8..0x30130028, 1360 байт).
#[link_section = ".origdata"]
#[used]
static PRODUCT_TABLE_BLOB: [u8; 1360] = *include_bytes!("../data/product_table.bin");

/// База таблицы в оригинальном образе (для пересчёта указателей).
const ORIG_TABLE_VA: usize = 0x3012_FAD8;

/// Разобранная запись продукта.
pub struct ProductInfo {
    pub id: u32,
    pub product_name: &'static [u8],
    pub name_plus: &'static [u8],
    pub dload_id: &'static [u8],
    pub hw_ver: &'static [u8],
    pub inner_name: &'static [u8],
    pub bands: [u32; 3],
}

/// Достать C-строку из блоба по ОРИГИНАЛЬНОМУ VA (пересчёт к базе блоба).
fn str_by_orig_va(va: u32) -> &'static [u8] {
    let off = (va as usize).wrapping_sub(ORIG_TABLE_VA);
    if off >= PRODUCT_TABLE_BLOB.len() {
        return b"";
    }
    let mut end = off;
    while end < PRODUCT_TABLE_BLOB.len() && PRODUCT_TABLE_BLOB[end] != 0 {
        end += 1;
    }
    &PRODUCT_TABLE_BLOB[off..end]
}

/// Найти запись по id (линейный проход, как BSP_ProductInfoInit).
pub fn find(id: u32) -> Option<ProductInfo> {
    for i in 0..17usize {
        let off = i * 0x34;
        if off + 0x24 > PRODUCT_TABLE_BLOB.len() {
            break;
        }
        let rec = &PRODUCT_TABLE_BLOB[off..];
        let rec_id = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
        if rec_id != id {
            continue;
        }
        let ptr = |o: usize| {
            u32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]])
        };
        return Some(ProductInfo {
            id: rec_id,
            product_name: str_by_orig_va(ptr(0x04)),
            name_plus: str_by_orig_va(ptr(0x08)),
            dload_id: str_by_orig_va(ptr(0x0C)),
            hw_ver: str_by_orig_va(ptr(0x10)),
            inner_name: str_by_orig_va(ptr(0x14)),
            bands: [ptr(0x18), ptr(0x1C), ptr(0x20)],
        });
    }
    None
}

/// ID продукта этой сборки: E3272s-153 (main=0x03, sub=0x01).
pub const PRODUCT_ID_E3272S_153: u32 = 0x0000_0301;

/// Активная запись продукта (E3272s-153; при отсутствии — дефолт MPWUDP).
pub fn current() -> ProductInfo {
    find(PRODUCT_ID_E3272S_153)
        .or_else(|| find(0xFFFF))
        .unwrap_or(ProductInfo {
            id: 0xFFFF,
            product_name: b"MPWUDP",
            name_plus: b"MPWUDP",
            dload_id: b"MPWUDP",
            hw_ver: b"MPWUDP",
            inner_name: b"MPWUDP",
            bands: [0, 0, 0],
        })
}

/// DLOAD-имя продукта (для 'E'-ответа DLOAD и getDloadID).
pub fn dload_name() -> Option<&'static [u8]> {
    let p = find(PRODUCT_ID_E3272S_153)?;
    Some(p.dload_id)
}

/// Маски бэндов (BSP_GetSupportBands: 3 слова GU/LTELow/LTEHigh).
pub fn support_bands() -> [u32; 3] {
    current().bands
}
