//! usb/dwc.rs — драйвер контроллера USB device: Synopsys DWC OTG HS.
//!
//! РЕКОНСТРУКЦИЯ ИЗ ОБРАЗА:
//!   * база контроллера — **0x900C0000** (конфиг-структура @0x30181C40:
//!     {base=0x900C0000, ...=0x26,0x26, 0x2012}); PHY — 0x900C0804;
//!   * регистровая карта — стандарт Synopsys DWC OTG (подтверждено
//!     обращениями dcd_syn_hs: DIEPCTL @0x900+ep*0x20, DIEPDMA @0x914 —
//!     FUN_30051ff4 и др.);
//!   * сброс/тактирование (USB_UnReset @0x30004A18):
//!       [0x9000000C] |= 0x08000000, |= 0x2000;  [0x90000038] &= ~0x1000;
//!       пауза; [0x900C0804] |= 2;  пауза;  [0x9000C000] = (val&~3)|2;
//!       [0x90000494] |= 0x02000000, |= 0x18000000;  [0x90000038] &= ~0x1000;
//!   * профили портов: burn-режим PID **0x1C05**, VID 0x12D1,
//!     3 порта CDC-ACM «MDM+PCUI+DIAG» (docs/03 §5, docs/08 §5).
//!
//! РЕЖИМ: device-only, FIFO (без DMA — функционально эквивалентно оригиналу
//! на уровне проводного протокола; оригинал использовал DMA-режим DIEPDMA).
//! Прерывания: линия USB (0x26=38 по конфигу @0x30181C58/5C —
//! подтверждено косвенно; в скелете также опрашиваем GINTSTS в главном
//! цикле для устойчивости).

use crate::mmio::*;

/// База DWC OTG.
pub const DWC_BASE: usize = 0x900C_0000;

// ---- Стандартные регистры DWC OTG (карта Synopsys) ----
const GOCTL: usize = 0x000; // GOTGCTL
const GAHBCFG: usize = 0x008;
const GUSBCFG: usize = 0x00C;
const GRSTCTL: usize = 0x010;
const GINTSTS: usize = 0x014;
const GINTMSK: usize = 0x018;
const GRXSTSP: usize = 0x020; // статус приёма (pop)
const GRXFSIZ: usize = 0x024; // глубина RX FIFO (слов)
const GNPTXFSIZ: usize = 0x028; // TX FIFO 0 (EP0 IN): [глубина|старт]
const GSNPSID: usize = 0x040; // идентификатор ядра
const GHWCFG2: usize = 0x048;
const GHWCFG4: usize = 0x050;
const DCFG: usize = 0x800;
const DCTL: usize = 0x804;
const DSTS: usize = 0x808;
const DIEPMSK: usize = 0x810;
const DOEPMSK: usize = 0x814;
const DAINT: usize = 0x818;
const DAINTMSK: usize = 0x81C;
const DIEPCTL0: usize = 0x900;
const DIEPINT0: usize = 0x908;
const DIEPTSIZ0: usize = 0x910;
const DOEPCTL0: usize = 0xB00;
const DOEPINT0: usize = 0xB08;
const DOEPTSIZ0: usize = 0xB10;
const DIEPCTL: usize = 0x900; // + ep*0x20
const DOEPCTL: usize = 0xB00; // + ep*0x20
const DIEPINT: usize = 0x908;
const DOEPINT: usize = 0xB08;
const DIEPTSIZ: usize = 0x910;
const DOEPTSIZ: usize = 0xB10;
const DIEPDMA: usize = 0x914; // (+0x914 в оригинале — подтверждено)
const DTKNQR2: usize = 0x820;
const PCGCTL: usize = 0xE00;

// ---- Биты ----
const GRSTCTL_CSFTRST: u32 = 1 << 0;
const GRSTCTL_AHBIDLE: u32 = 1 << 31;
const GINTSTS_USBRST: u32 = 1 << 12;
const GINTSTS_ENUMDONE: u32 = 1 << 13;
const GINTSTS_USBSUSP: u32 = 1 << 11;
const GINTSTS_WKUPINT: u32 = 1 << 10;
const GINTSTS_RXFLVL: u32 = 1 << 4;
const GINTSTS_IEPINT: u32 = 1 << 18;
const GINTSTS_OEPINT: u32 = 1 << 19;
const GINTSTS_GOUTNAKEFF: u32 = 1 << 7;
const DCFG_DEVADDR_MASK: u32 = 0x7F0;
const DCTL_SFTDISCON: u32 = 1 << 1;
const DIEPCTL_EPENA: u32 = 1 << 31;
const DIEPCTL_EPDIS: u32 = 1 << 30;
const DIEPCTL_SNAK: u32 = 1 << 27;
const DIEPCTL_CNAK: u32 = 1 << 26;
const DIEPCTL_EPTYPE_BULK: u32 = 2 << 18;
const DIEPCTL_EPTYPE_INT: u32 = 3 << 18;
const DIEPCTL_SETD0PID: u32 = 1 << 28;
const DIEPCTL_STALL: u32 = 1 << 21;
const DOEPCTL_EPENA: u32 = 1 << 31;
const DOEPCTL_CNAK: u32 = 1 << 26;
const DOEPCTL_SNAK: u32 = 1 << 27;
const DOEPCTL_STALL: u32 = 1 << 21;

/// Число OUT-эндпоинтов (заполняется из GHWCFG4).
static mut NUM_IN_EPS: u32 = 4;
static mut NUM_OUT_EPS: u32 = 4;
/// Адрес устройства (после SET_ADDRESS).
static mut DEV_ADDR: u32 = 0;
/// Скорость после enum (0=HS).
static mut ENUM_SPEED: u32 = 0;

/// Сброс/тактирование контроллера и PHY (USB_UnReset, байт-в-байт порядок).
pub fn unreset() {
    unsafe {
        // *0x9000000C: бит27 → 1, затем бит13 → 1
        rmw32(0x9000_000C, 0x0800_0000, 0x0800_0000);
        rmw32(0x9000_000C, 0x0000_2000, 0x0000_2000);
        // *0x90000038 &= ~0x1000
        rmw32(0x9000_0038, 0x0000_1000, 0);
        delay_ms(2); // taskDelay(2)
        // *0x900C0804 |= 2 (PHY)
        rmw32(0x900C_0804, 0x2, 0x2);
        delay_ms(2);
        // *0x9000C000 = (v & ~3) | 2
        rmw32(0x9000_C000, 0x3, 0x2);
        // *0x90000494 |= 0x02000000, |= 0x18000000
        rmw32(0x9000_0494, 0x0200_0000, 0x0200_0000);
        rmw32(0x9000_0494, 0x1800_0000, 0x1800_0000);
        // *0x90000038 &= ~0x1000 (повтор)
        rmw32(0x9000_0038, 0x0000_1000, 0);
    }
}

/// Инициализация ядра: soft reset, конфигурация, FIFO, прерывания.
pub fn core_init() -> bool {
    unsafe {
        let id = read32(DWC_BASE + GSNPSID);
        if id & 0xFFF0_0000 != 0x4F54_0000 {
            // не DWC OTG — контроллер не отвечает
            crate::console::puts("USB: GSNPSID=");
            crate::console::put_u32_hex(id, 8);
            crate::console::putln(" — не DWC OTG");
            return false;
        }

        // soft reset ядра
        write32(DWC_BASE + GRSTCTL, GRSTCTL_CSFTRST);
        let mut timeout = 100_000u32;
        while read32(DWC_BASE + GRSTCTL) & GRSTCTL_CSFTRST != 0 {
            if timeout == 0 {
                return false;
            }
            timeout -= 1;
        }
        // ожидание AHB idle
        timeout = 100_000;
        while read32(DWC_BASE + GRSTCTL) & GRSTCTL_AHBIDLE == 0 {
            if timeout == 0 {
                return false;
            }
            timeout -= 1;
        }

        // геометрия ядра
        let hwcfg4 = read32(DWC_BASE + GHWCFG4);
        NUM_IN_EPS = ((hwcfg4 >> 26) & 0xF) + 1;
        NUM_OUT_EPS = ((hwcfg4 >> 20) & 0xF) + 1;

        // GAHBCFG: без DMA (бит5=0), глобальное разрешение прерываний (бит0)
        write32(DWC_BASE + GAHBCFG, 0x1);

        // GUSBCFG: 16-бит UTMI, HNP/SRP off; ForceDeviceMode (бит30 —
        // для ядер с dual-role; безопасно для device-only)
        write32(DWC_BASE + GUSBCFG, 0x0000_1000 | (1 << 30));

        // устройство: сбросить адрес, скорость — любая (DCFG devspd: 0=HS)
        write32(DWC_BASE + DCFG, 0x0);

        // FIFO: RX 256 слов, TX0 (EP0) 64 слова с адреса 256,
        // TX1..TX6 по 128 слов (bulk 512Б = 128 слов + запас)
        write32(DWC_BASE + GRXFSIZ, 256);
        write32(DWC_BASE + GNPTXFSIZ, (64 << 16) | 256);
        let mut tx_start = 256 + 64;
        for ep in 1..=6usize {
            let off = 0x100 + (ep - 1) * 4; // DIEPTXFSIZ(n)
            write32(DWC_BASE + off, (128 << 16) | tx_start);
            tx_start += 128;
        }

        // сбросить soft-disconnect
        rmw32(DWC_BASE + DCTL, DCTL_SFTDISCON, 0);

        // маски прерываний: reset/enumdone/rxflvl/eps/suspend
        write32(
            DWC_BASE + GINTMSK,
            GINTSTS_USBRST
                | GINTSTS_ENUMDONE
                | GINTSTS_RXFLVL
                | GINTSTS_IEPINT
                | GINTSTS_OEPINT
                | GINTSTS_USBSUSP,
        );
        // разблокировать прерывания эндпоинтов
        write32(DWC_BASE + DAINTMSK, 0xFFFF_FFFF);
        write32(DWC_BASE + DIEPMSK, 0x0F);
        write32(DWC_BASE + DOEPMSK, 0x0F);

        // подключиться к шине (снять soft disconnect уже сделано)
        true
    }
}

/// Значение скорости после ENUMDONE: биты 17..19 DSTS.
pub fn enum_speed() -> u32 {
    unsafe { ENUM_SPEED }
}

/// Установить адрес (SET_ADDRESS → DCFG).
pub fn set_address(addr: u32) {
    unsafe {
        DEV_ADDR = addr;
        let dcfg = read32(DWC_BASE + DCFG) & !DCFG_DEVADDR_MASK;
        write32(DWC_BASE + DCFG, dcfg | ((addr & 0x7F) << 4));
    }
}

/// Поллинговый обработчик событий (вызывается из главного цикла; те же
/// действия выполняет и irq-путь). Возврат — был ли reset.
pub fn poll() -> bool {
    let mut had_reset = false;
    unsafe {
        let sts = read32(DWC_BASE + GINTSTS);
        if sts == 0 {
            return false;
        }
        write32(DWC_BASE + GINTSTS, sts); // сбросить флаги (W1C)

        if sts & GINTSTS_USBRST != 0 {
            bus_reset();
            had_reset = true;
        }
        if sts & GINTSTS_ENUMDONE != 0 {
            enum_done();
        }
        if sts & GINTSTS_RXFLVL != 0 {
            handle_rx_fifo();
        }
        if sts & GINTSTS_IEPINT != 0 {
            handle_in_eps();
        }
        if sts & GINTSTS_OEPINT != 0 {
            handle_out_eps();
        }
    }
    had_reset
}

/// Обработка шинного сброса: выключить все EP, сбросить адрес.
unsafe fn bus_reset() {
    DEV_ADDR = 0;
    let dcfg = read32(DWC_BASE + DCFG) & !DCFG_DEVADDR_MASK;
    write32(DWC_BASE + DCFG, dcfg);
    // выключить/занакать все EP кроме EP0
    for ep in 1..(NUM_IN_EPS as usize) {
        let ctl = read32(DWC_BASE + DIEPCTL + ep * 0x20);
        if ctl & DIEPCTL_EPENA != 0 {
            write32(DWC_BASE + DIEPCTL + ep * 0x20, DIEPCTL_EPDIS | DIEPCTL_SNAK);
        }
    }
    for ep in 1..(NUM_OUT_EPS as usize) {
        let ctl = read32(DWC_BASE + DOEPCTL + ep * 0x20);
        if ctl & DOEPCTL_EPENA != 0 {
            write32(DWC_BASE + DOEPCTL + ep * 0x20, DOEPCTL_SNAK);
        }
    }
    crate::usb::ch9::on_bus_reset();
    // подготовить EP0 OUT к приёму setup
    ep0_out_setup();
}

/// ENUMDONE: зафиксировать скорость и поднять EP0.
unsafe fn enum_done() {
    let dsts = read32(DWC_BASE + DSTS);
    ENUM_SPEED = (dsts >> 17) & 7;
    crate::usb::ch9::on_enum_done(enum_speed());
    ep0_out_setup();
    // NAK EP0 IN до первого запроса
    write32(DWC_BASE + DIEPCTL0, DIEPCTL_SNAK);
}

/// Подготовить EP0 OUT к приёму пакета SETUP (3 слова).
unsafe fn ep0_out_setup() {
    write32(DWC_BASE + DOEPTSIZ0, (3 << 19) | 64); // 3 пакета x 64 байта
    write32(DWC_BASE + DOEPCTL0, DOEPCTL_CNAK | DOEPCTL_EPENA);
}

/// Чтение GRXSTSP и диспетчеризация приёма.
unsafe fn handle_rx_fifo() {
    let mut guard = 0;
    while guard < 64 {
        guard += 1;
        let pop = read32(DWC_BASE + GRXSTSP);
        let pktsts = (pop >> 17) & 0xF;
        let epnum = (pop >> 0) & 0xF;
        let bcnt = ((pop >> 4) & 0x7FF) as usize;

        match pktsts {
            1 => {
                // OUT packet на EP
                read_rx_fifo(epnum as usize, bcnt);
            }
            2 => {
                // OUT transfer completed
                crate::usb::ch9::on_out_complete(epnum as usize);
                if epnum == 0 {
                    ep0_out_setup();
                }
            }
            3 => {
                // OUT transfer completed (последний пакет)
                // данные уже прочитаны в case 1
            }
            4 => {
                // SETUP transaction complete
                let mut setup = [0u8; 8];
                for i in 0..2 {
                    let w = read_rx_word();
                    setup[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
                }
                crate::usb::ch9::on_setup(&setup);
            }
            5 => {
                // SETUP transaction data error
                ep0_out_setup();
            }
            6 | 7 => {
                // global OUT NAK / SRP — игнорируем
            }
            _ => {}
        }
        if read32(DWC_BASE + GINTSTS) & GINTSTS_RXFLVL == 0 {
            break;
        }
    }
}

/// Чтение слова из RX FIFO: стандартный адрес FIFO0 (приём) — база + 0x1000.
unsafe fn read_rx_word() -> u32 {
    read32(DWC_BASE + 0x1000)
}

/// Чтение bcnt байт из RX FIFO в буфер EP (выравнивание до слов).
unsafe fn read_rx_fifo(ep: usize, bcnt: usize) {
    let words = (bcnt + 3) / 4;
    let mut tmp = [0u8; 4];
    for w in 0..words {
        let v = read_rx_word();
        tmp.copy_from_slice(&v.to_le_bytes());
        let base = w * 4;
        let take = (bcnt - base).min(4);
        crate::usb::ch9::on_out_data(ep, &tmp[..take]);
    }
}

/// Обработка прерываний IN-эндпоинтов (завершение передачи).
unsafe fn handle_in_eps() {
    let daint = read32(DWC_BASE + DAINT) & 0xFFFF; // IN-половина
    for ep in 0..(NUM_IN_EPS as usize) {
        if daint & (1 << ep) == 0 {
            continue;
        }
        let int = read32(DWC_BASE + DIEPINT + ep * 0x20);
        write32(DWC_BASE + DIEPINT + ep * 0x20, int); // W1C
        if int & (1 << 0) != 0 {
            // XFRC — передача завершена
            crate::usb::ch9::on_in_complete(ep);
        }
    }
}

/// Обработка прерываний OUT-эндпоинтов.
unsafe fn handle_out_eps() {
    let daint = read32(DWC_BASE + DAINT) >> 16; // OUT-половина
    for ep in 0..(NUM_OUT_EPS as usize) {
        if daint & (1 << ep) == 0 {
            continue;
        }
        let int = read32(DWC_BASE + DOEPINT + ep * 0x20);
        write32(DWC_BASE + DOEPINT + ep * 0x20, int);
        if int & (1 << 0) != 0 {
            crate::usb::ch9::on_out_complete(ep);
        }
    }
}

/// Запись данных в TX FIFO EP (IN). Автоматика пакетирования.
pub fn ep_write(ep: usize, data: &[u8]) -> bool {
    unsafe {
        if ep >= NUM_IN_EPS as usize {
            return false;
        }
        // скопировать в FIFO EP
        let fifo = DWC_BASE + 0x1000 + ep * 0x1000; // TX FIFO ep
        let mut i = 0;
        while i < data.len() {
            let mut w = [0u8; 4];
            let take = (data.len() - i).min(4);
            w[..take].copy_from_slice(&data[i..i + take]);
            write32(fifo, u32::from_le_bytes(w));
            i += take;
        }
        // настроить передачу: пакеты по 64 (HS EP0) / 512 (bulk HS)
        let maxpkt: u32 = if ep == 0 {
            64
        } else if enum_speed() == 0 {
            512
        } else {
            64
        };
        let pkts = (data.len() as u32 + maxpkt - 1) / maxpkt;
        let sizereg = if ep == 0 { DIEPTSIZ0 } else { DIEPTSIZ + ep * 0x20 };
        write32(DWC_BASE + sizereg, (pkts << 19) | data.len() as u32);
        // разрешить передачу + CNAK
        let ctlreg = if ep == 0 { DIEPCTL0 } else { DIEPCTL + ep * 0x20 };
        let mut ctl = read32(DWC_BASE + ctlreg) & !(0x3 << 28);
        if ep != 0 {
            // тип endpoинта задаётся при конфигурации (ch9)
            ctl |= DIEPCTL_SETD0PID;
        }
        write32(DWC_BASE + ctlreg, ctl | DIEPCTL_EPENA | DIEPCTL_CNAK);
        true
    }
}

/// Stall на EP0 (ошибка запроса CH9).
pub fn ep0_stall() {
    unsafe {
        write32(DWC_BASE + DIEPCTL0, DIEPCTL_STALL);
        write32(DWC_BASE + DOEPCTL0, DOEPCTL_STALL);
    }
}

/// Настроить bulk/INT эндпоинт после SET_CONFIGURATION (вызывается из ch9).
pub fn ep_configure(ep: usize, is_in: bool, is_int: bool) {
    unsafe {
        let maxpkt: u32 = if is_int { 64 } else if enum_speed() == 0 { 512 } else { 64 };
        if is_in {
            let ctl = read32(DWC_BASE + DIEPCTL + ep * 0x20);
            let typ = if is_int { DIEPCTL_EPTYPE_INT } else { DIEPCTL_EPTYPE_BULK };
            write32(
                DWC_BASE + DIEPCTL + ep * 0x20,
                (ctl & !(0x3 << 18)) | typ | maxpkt | DIEPCTL_SETD0PID | DIEPCTL_SNAK,
            );
        } else {
            let ctl = read32(DWC_BASE + DOEPCTL + ep * 0x20);
            let typ = if is_int { 3 << 18 } else { 2 << 18 };
            write32(
                DWC_BASE + DOEPCTL + ep * 0x20,
                (ctl & !(0x3 << 18)) | typ | maxpkt | (1 << 28),
            );
            ep_out_arm(ep, 512);
        }
    }
}

/// Вооружить OUT-эндпоинт на приём (один пакет до maxpkt).
pub fn ep_out_arm(ep: usize, maxpkt: u32) {
    unsafe {
        let sizereg = if ep == 0 { DOEPTSIZ0 } else { DOEPTSIZ + ep * 0x20 };
        let ctlreg = if ep == 0 { DOEPCTL0 } else { DOEPCTL + ep * 0x20 };
        write32(DWC_BASE + sizereg, maxpkt | (1 << 19));
        let ctl = read32(DWC_BASE + ctlreg);
        write32(DWC_BASE + ctlreg, ctl | DOEPCTL_EPENA | DOEPCTL_CNAK);
    }
}

/// Послать статус-этап EP0 (zero-length packet).
pub fn ep0_send_status() {
    ep_write(0, &[]);
}
