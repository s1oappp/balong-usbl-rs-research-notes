//! ptable.rs — таблица разделов NAND (ptable).
//!
//! РЕКОНСТРУКЦИЯ (docs/06, docs/11, функции ptable_* из flash_03/04.c):
//!   * запись 0x30 байт: имя[16], offset, ?, size, load_addr, addr2/exec,
//!     type, flags (bit3 = валидность активной копии), seq (поколение A/B);
//!   * конец таблицы: маркер `T` (type 0x200, flags 0x1) и `pTableTail`;
//!   * в RAM дефолтная таблица @0x3018000C (0x400 байт), при финализации
//!     DLOAD пишется во flash @0x1E000 (ptable_replace_flash_table);
//!   * выбор копии при загрузке: ptable_find_by_type — фильтр validity,
//!     максимум seq; запись при прошивке: ptable_find_older_part — минимум
//!     seq, после записи older.seq = newer.seq + 1, переключение bit3;
//!   * принятие новой таблицы: type 0x100 в пакете, merge через
//!     ptable_replace_ram_table (защита отката версии);
//!   * integrality: bit0 слова head+0x0C (ptable_set_integrality).
//!
//! Дефолтная таблица ниже — точные 0x400 байт из образа (17 разделов).

/// Дефолтная ptable, извлечённая из образа @0x3018000C (1024 байта).
#[link_section = ".origdata"]
#[used]
static PTABLE_BLOB: [u8; 0x400] = *include_bytes!("../data/ptable.bin");

/// Смещение ptable во flash (dloadBootromEnd → ptable_replace_flash_table).
pub const PTABLE_FLASH_OFFSET: u32 = 0x1E000;

/// Размер записи.
pub const ENTRY_SIZE: usize = 0x30;
/// Максимальное число записей (0x400 / 0x30 = 21, из них 17 реальные).
pub const MAX_ENTRIES: usize = 21;

/// Коды типов разделов (docs/11 §2, сверка с balongflash/ptable.c).
pub mod types {
    pub const bootloader: u32 = 0x101;    // BootLoad (USBL)
    pub const bootrom: u32 = 0x102;       // BootRom (A/B)
    pub const vxworks: u32 = 0x103;       // VxWorks-модем
    pub const fastboot: u32 = 0x104;      // FastBoot
    pub const kernel: u32 = 0x105;        // Linux-ядро
    pub const yaffs0: u32 = 0x106;        // /yaffs0
    pub const system: u32 = 0x107;        // /yaffs1 System_R1
    pub const yaffs2: u32 = 0x108;        // /yaffs2
    pub const app: u32 = 0x109;           // /yaffs3 APP_R1
    pub const cdromiso: u32 = 0x10C;      // cdromiso
    pub const nv_backup_lte: u32 = 0x10D;  // NvBackLTE
    pub const nv_backup_gu: u32 = 0x10E;   // NvBackGU
    pub const logo: u32 = 0x110;          // Logo
    pub const yaffs5: u32 = 0x111;        // /yaffs5
    pub const yaffs4: u32 = 0x112;        // /yaffs4
    pub const webui: u32 = 0x113;         // /yaffs6 WEBUI_R1
    /// Специальный тип пакета: новая ptable (docs/10 §1.4).
    pub const new_ptable: u32 = 0x100;
    /// Маркер конца «T».
    pub const tail_t: u32 = 0x200;
}

/// Раздел ptable.
#[derive(Clone, Copy)]
pub struct PtableEntry {
    pub name: [u8; 16],
    pub offset: u32,
    pub unk: u32,
    pub size: u32,
    pub load_addr: u32,
    pub exec_addr: u32,
    pub kind: u32,
    pub flags: u32,
    pub seq: u32,
}

impl PtableEntry {
    fn from_blob(b: &[u8]) -> Self {
        let mut name = [0u8; 16];
        name.copy_from_slice(&b[..16]);
        let w = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        PtableEntry {
            name,
            offset: w(0x10),
            unk: w(0x14),
            size: w(0x18),
            load_addr: w(0x1C),
            exec_addr: w(0x20),
            kind: w(0x24),
            flags: w(0x28),
            seq: w(0x2C),
        }
    }

    /// Бит валидности (bit3 поля flags).
    pub fn valid(&self) -> bool {
        self.flags & 0x8 != 0
    }

    /// Имя раздела как &str-сырые байты (до NUL).
    pub fn name_bytes(&self) -> &[u8] {
        let len = self.name.iter().position(|&c| c == 0).unwrap_or(16);
        &self.name[..len]
    }
}

/// Рабочая копия таблицы в RAM (аналог RAM-таблицы оригинала @0x3018000C).
pub static mut RAM_TABLE: [u8; 0x400] = [0; 0x400];

/// Загрузить дефолтную таблицу в RAM-копию (вызывается при старте).
pub fn init() {
    unsafe {
        RAM_TABLE.copy_from_slice(&PTABLE_BLOB);
    }
}

/// Число записей до маркера `T`/`pTableTail`.
pub fn count() -> usize {
    let t = unsafe { &RAM_TABLE };
    for i in 0..MAX_ENTRIES {
        let off = i * ENTRY_SIZE;
        if off + ENTRY_SIZE > t.len() {
            return i;
        }
        let kind = u32::from_le_bytes([
            t[off + 0x24],
            t[off + 0x25],
            t[off + 0x26],
            t[off + 0x27],
        ]);
        if kind >= types::tail_t || kind == 0 {
            return i;
        }
    }
    MAX_ENTRIES
}

/// Запись по индексу.
pub fn entry(i: usize) -> Option<PtableEntry> {
    if i >= count() {
        return None;
    }
    let t = unsafe { &RAM_TABLE };
    Some(PtableEntry::from_blob(&t[i * ENTRY_SIZE..]))
}

/// Найти ЛУЧШУЮ (валидную, макс. seq) запись по типу — ptable_find_by_type.
pub fn find_by_type(kind: u32) -> Option<PtableEntry> {
    let n = count();
    let mut best: Option<PtableEntry> = None;
    for i in 0..n {
        let e = entry(i)?;
        if e.kind != kind || !e.valid() {
            continue;
        }
        if best.map_or(true, |b| e.seq > b.seq) {
            best = Some(e);
        }
    }
    best
}

/// Найти СТАРУЮ (мин. seq, игнорируя validity) копию раздела для записи —
/// ptable_find_older_part (A/B-схема: пишем в неактивную копию).
pub fn find_older_part(kind: u32) -> Option<(usize, PtableEntry)> {
    let n = count();
    let mut best: Option<(usize, PtableEntry)> = None;
    for i in 0..n {
        let e = entry(i)?;
        if e.kind != kind {
            continue;
        }
        if best.map_or(true, |(_, b)| e.seq < b.seq) {
            best = Some((i, e));
        }
    }
    best
}

/// Обновить счётчик поколений и валидность после записи (ImageDloadToFlash):
/// older.seq = newer.seq + 1; переключить bit3.
pub fn activate_copy(index: usize) {
    let t = unsafe { &mut RAM_TABLE };
    let off = index * ENTRY_SIZE;
    if off + ENTRY_SIZE > t.len() {
        return;
    }
    let kind = u32::from_le_bytes([
        t[off + 0x24],
        t[off + 0x25],
        t[off + 0x26],
        t[off + 0x27],
    ]);
    // максимум seq среди всех копий этого типа
    let mut max_seq = 0u32;
    for i in 0..count() {
        if let Some(e) = entry(i) {
            if e.kind == kind && e.seq > max_seq {
                max_seq = e.seq;
            }
        }
    }
    let new_seq = max_seq.wrapping_add(1);
    t[off + 0x2C..off + 0x30].copy_from_slice(&new_seq.to_le_bytes());
    let flags = u32::from_le_bytes([
        t[off + 0x28],
        t[off + 0x29],
        t[off + 0x2A],
        t[off + 0x2B],
    ]) | 0x8;
    t[off + 0x28..off + 0x2C].copy_from_slice(&flags.to_le_bytes());
    // у второй копии снять валидность
    for i in 0..count() {
        if i == index {
            continue;
        }
        if let Some(e) = entry(i) {
            if e.kind == kind {
                let o = i * ENTRY_SIZE;
                let f = u32::from_le_bytes([
                    t[o + 0x28],
                    t[o + 0x29],
                    t[o + 0x2A],
                    t[o + 0x2B],
                ]) & !0x8;
                t[o + 0x28..o + 0x2C].copy_from_slice(&f.to_le_bytes());
            }
        }
    }
}

/// Флаг целостности таблицы (ptable_set_integrality @0x3002BB10 — bit0
/// поля +0x0C заголовка ptable; формат флеш-заголовка в репозитории-
/// реконструкции полностью не восстановлен, поэтому флаг хранится рядом
/// с RAM-копией и устанавливается при финализации).
static mut INTEGRALITY: bool = false;

/// Установить флаг целостности (ptable_set_integrality).
pub fn set_integrality(ok: bool) {
    unsafe {
        INTEGRALITY = ok;
    }
}

/// Проверить флаг целостности (ptable_check_integrality @0x3002BAEC).
pub fn check_integrality() -> bool {
    unsafe { INTEGRALITY }
}

/// Записать RAM-таблицу во flash @0x1E000 (ptable_replace_flash_table).
pub fn replace_flash_table() -> bool {
    crate::nand::write(PTABLE_FLASH_OFFSET, unsafe { &RAM_TABLE })
}

/// Взять RAM-таблицу (для отладки/дампа).
pub fn ram_table() -> &'static [u8; 0x400] {
    unsafe { &RAM_TABLE }
}

/// Мутабельная RAM-таблица (для merge новой ptable).
pub fn ram_table_mut() -> &'static mut [u8; 0x400] {
    unsafe { &mut RAM_TABLE }
}
