//! hdlc.rs — HDLC-кадрирование DLOAD-протокола (модуль Om_* оригинала).
//!
//! РЕКОНСТРУКЦИЯ БАЙТ-В-БАЙТ (Om_HdlcDecap @0x30085B80, Om_HdlcEncap
//! @0x30085888, OM_HdlcFcs @0x30085B34, Om_HdlcInit @0x30085AFC):
//!   * флаг кадра 0x7E; экранирование 0x7D, экранированный байт XOR 0x20;
//!     после 0x7D допустимы только 0x5E и 0x5D, иначе сброс (ошибка 3);
//!   * FCS-16 PPP (RFC 1662): init 0xFFFF, отражённый полином 0x8408,
//!     таблица 256xu16 из образа @0x3014C108 (встроена байт-в-байт ниже),
//!     «магия» валидного кадра 0xF0B8;
//!   * FCS передаётся дополнением до единицы, младшим байтом вперёд;
//!   * кадр = [0x7E] payload FCS(2) [0x7E]; между флагами ≥ 3 байт
//!     (иначе кадр игнорируется), полезная длина = длина-2;
//!   * максимальный буфер приёма 0x2200 (8704) байт.
//!
//! Коды возврата decap: 0 = кадр готов, 1 = в процессе, 2 = ошибка CRC,
//! 3 = ошибка экранирования, 4 = переполнение, 5 = не инициализировано.

/// Таблица FCS-16 (256 × u16 = 512 байт), извлечённая из оригинального
/// образа @0x3014C108 байт-в-байт (стандартные значения RFC 1662 —
/// совпадение подтверждено сверкой).
#[link_section = ".origdata"]
#[used]
static FCS16_TABLE_BYTES: [u8; 512] = *include_bytes!("../data/fcs16_table.bin");

/// Доступ к таблице как к u16 (выравнивание: .origdata выровнен на 4).
#[inline]
fn fcs_table_entry(i: usize) -> u32 {
    let b = &FCS16_TABLE_BYTES[i * 2..i * 2 + 2];
    u16::from_le_bytes([b[0], b[1]]) as u32
}

/// Начальное значение FCS (DAT_30085d70 = 0xFFFF).
pub const FCS_INIT: u32 = 0xFFFF;
/// Значение FCS после обработки корректного кадра (DAT_30085d9c = 0xF0B8).
pub const FCS_GOOD: u32 = 0xF0B8;
/// Максимальный размер буфера приёма (как в FUN_30080798: 0x2200).
pub const HDLC_MAX_FRAME: usize = 0x2200;

/// Расчёт FCS по байтам (OM_HdlcFcs: таблица + сдвиг).
#[inline]
pub fn fcs16(bytes: &[u8]) -> u32 {
    let mut fcs = FCS_INIT;
    for &b in bytes {
        fcs = fcs_table_entry(((fcs ^ b as u32) & 0xFF) as usize) ^ (fcs >> 8);
    }
    fcs & 0xFFFF
}

/// Коды возврата Om_HdlcDecap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecapResult {
    /// Кадр принят, полезная длина в state.frame_len.
    FrameReady = 0,
    /// Приём продолжается.
    InProgress = 1,
    /// Ошибка CRC.
    CrcError = 2,
    /// Ошибка экранирования.
    EscapeError = 3,
    /// Переполнение буфера.
    Overflow = 4,
    /// Не инициализировано.
    NotReady = 5,
}

/// Состояние приёмника — раскладка слов как у оригинального Om (5 слов):
/// +0 флаги (bit0 — вне кадра/сброс, bit1 — увидели 0x7D), +4 длина,
/// +8 макс. длина, +0xC буфер, +0x10 длина полезной части кадра.
pub struct HdlcState {
    flags: u32,
    len: usize,
    max_len: usize,
    buf: *mut u8,
    /// Длина полезной части последнего принятого кадра.
    pub frame_len: usize,
}

impl HdlcState {
    /// Om_HdlcInit: flags=1 (ждать флага), len=0.
    pub fn new(buf: *mut u8, max_len: usize) -> Self {
        HdlcState {
            flags: 1,
            len: 0,
            max_len,
            buf,
            frame_len: 0,
        }
    }

    /// Обработка одного байта — точная семантика Om_HdlcDecap.
    pub fn decap(&mut self, byte: u8) -> DecapResult {
        if self.max_len == 0 || self.buf.is_null() {
            return DecapResult::NotReady;
        }
        if (self.flags & 1) == 0 || byte == 0x7E {
            if byte == 0x7D {
                if self.flags & 2 != 0 {
                    // 0x7D после 0x7D — сброс
                    self.flags = (self.flags & !3) | 1;
                    self.len = 0;
                    return DecapResult::EscapeError;
                }
                self.flags |= 2;
            } else if byte == 0x7E {
                self.flags &= !1;
                if self.len > 2 {
                    let fcs = fcs16(unsafe {
                        core::slice::from_raw_parts(self.buf, self.len)
                    });
                    if fcs != FCS_GOOD {
                        self.len = 0;
                        return DecapResult::CrcError;
                    }
                    // кадр валиден: последние 2 байта — FCS
                    self.frame_len = self.len - 2;
                    self.len = 0;
                    return DecapResult::FrameReady;
                }
                self.len = 0;
            } else {
                // байт данных
                if self.len >= self.max_len {
                    self.flags |= 1;
                    self.len = 0;
                    return DecapResult::Overflow;
                }
                if self.flags & 2 != 0 {
                    self.flags &= !2;
                    if byte != 0x5E && byte != 0x5D {
                        self.flags |= 1;
                        self.len = 0;
                        return DecapResult::EscapeError;
                    }
                    let unescaped = byte ^ 0x20;
                    unsafe { *self.buf.add(self.len) = unescaped };
                } else {
                    unsafe { *self.buf.add(self.len) = byte };
                }
                self.len += 1;
            }
            DecapResult::InProgress
        } else {
            // вне кадра, не флаг — ресинхронизация длины
            self.len = 0;
            DecapResult::InProgress
        }
    }

    /// Ссылка на буфер принятого кадра.
    pub fn frame(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.buf, self.frame_len) }
    }

    /// Мутабельная ссылка (для обработки на месте).
    pub fn frame_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.buf, self.frame_len) }
    }
}

/// Инкапсуляция кадра (Om_HdlcEncap): [0x7E] данные FCS [0x7E].
/// Возвращает длину записанного в out или None при нехватке места.
pub fn encap(payload: &[u8], out: &mut [u8]) -> Option<usize> {
    if out.len() < 5 + payload.len() {
        return None; // минимум: флаг + 2 FCS-байта (экранированные до 4) + флаг
    }
    let mut fcs = FCS_INIT;
    let mut o = 0;
    out[o] = 0x7E;
    o += 1;
    for &b in payload {
        fcs = fcs_table_entry(((fcs ^ b as u32) & 0xFF) as usize) ^ (fcs >> 8);
        if b == 0x7E || b == 0x7D {
            out[o] = 0x7D;
            out[o + 1] = b ^ 0x20;
            o += 2;
        } else {
            out[o] = b;
            o += 1;
        }
    }
    let fcs = !fcs & 0xFFFF;
    // младший байт вперёд, каждый может потребовать экранирования
    for b in [ (fcs & 0xFF) as u8, (fcs >> 8) as u8 ] {
        if b == 0x7E || b == 0x7D {
            out[o] = 0x7D;
            out[o + 1] = b ^ 0x20;
            o += 2;
        } else {
            out[o] = b;
            o += 1;
        }
    }
    out[o] = 0x7E;
    o += 1;
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fcs_table_matches_rfc1662() {
        // первое/второе/последнее значения эталонной таблицы PPP FCS-16
        assert_eq!(fcs_table_entry(0), 0x0000);
        assert_eq!(fcs_table_entry(1), 0x1189);
        assert_eq!(fcs_table_entry(255), 0x0F78);
    }

    #[test]
    fn roundtrip() {
        let payload = [0x41u8, 0x00, 0x00, 0x30, 0x00, 0x10, 0x00, 0x7E, 0x7D];
        let mut out = [0u8; 64];
        let n = encap(&payload, &mut out).unwrap();
        let mut buf = [0u8; 64];
        let mut st = HdlcState::new(buf.as_mut_ptr(), 64);
        let mut ready = false;
        for &b in &out[..n] {
            if st.decap(b) == DecapResult::FrameReady {
                ready = true;
            }
        }
        assert!(ready);
        assert_eq!(st.frame(), &payload);
    }

    #[test]
    fn known_frame() {
        // статический кадр ACK оригинала: 7e 02 6a d3 7e
        let mut buf = [0u8; 16];
        let mut st = HdlcState::new(buf.as_mut_ptr(), 16);
        for &b in &[0x7Eu8, 0x02, 0x6A, 0xD3, 0x7E] {
            st.decap(b);
        }
        assert_eq!(st.frame_len, 1);
        assert_eq!(st.frame()[0], 0x02);
    }
}
