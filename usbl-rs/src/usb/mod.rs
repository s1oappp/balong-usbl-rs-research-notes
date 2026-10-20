//! usb/mod.rs — стек USB: запуск, маршрутизация портов, DLOAD-канал.
//!
//! ОРИГИНАЛ: tUSBTask (entry 0x300048B0 → jusb_entry → jstart_stack →
//! jslave_init → dcd_syn_hs), CDC/ACM-функции, BSP_USB_ACM_Sio — мост в
//! VxWorks SIO; DLOAD-задача открывает VCOM (BSP_DLOAD_Init) и ставит
//! колбэк dloadUSBCallback. Здесь: драйвер dwc.rs + ch9.rs + desc.rs,
//! единый главный цикл (модель «голое железо»).

pub mod ch9;
pub mod desc;
pub mod dwc;

use crate::console;

/// Инициализация стека (USB_UnReset + core_init — эквивалент
/// USB_UnReset(); taskSpawn(tUSBTask) → jstart_stack → dcd).
pub fn init() -> bool {
    dwc::unreset();
    if !dwc::core_init() {
        console::putln("USB: инициализация DWC OTG не удалась");
        return false;
    }
    console::putln("USB: DWC OTG HS поднят (VID 0x12D1 PID 0x1C05)");
    true
}

/// Поллинг событий (из главного цикла).
pub fn poll() -> bool {
    dwc::poll()
}

/// Доставка принятых данных порта в единый протокол AT/DLOAD
/// (dloadUSBCallback-эквивалент; оригинал вешает его на VCOM №1).
pub fn deliver_to_protocol(_port: usize, data: &[u8]) {
    crate::at::usb_rx(data);
}

/// Отправка данных хосту из DLOAD-канала (dloadUSBSend →
/// BSP_DLOAD_SendData → ioctl VCOM). Здесь: bulk IN порта PCUI (EP2),
/// при занятости — MDM (EP1), затем DIAG (EP3) — надёжность доставки.
pub fn dload_send(data: &[u8]) {
    // выбрать свободный bulk IN EP: 2 (PCUI), 1 (MDM), 3 (DIAG)
    for ep in [2usize, 1, 3] {
        if dwc::ep_write(ep, data) {
            return;
        }
    }
}

/// Bulk IN-передача завершена (данные ушли).
pub fn on_tx_complete(_ep: usize) {}
