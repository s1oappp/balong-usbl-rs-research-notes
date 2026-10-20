//! usb/ch9.rs — Chapter 9 (стандартные запросы EP0) и маршрутизация данных.
//!
//! ОРИГИНАЛ: jusb_chp9 (chp9_handle_usb_command @0x3004D170 — диспетчер
//! EP0), cdc_fd/acm_fd (acm_handle_request @0x3005E9EC). Шаблоны
//! дескрипторов — см. desc.rs. Запросы SET_LINE_CODING/GET_LINE_CODING
//! и SEND_ENCAPSULATED_COMMAND принимаются CDC-ACM портовыми.

use super::desc;
use super::dwc;

/// Состояние EP0: текущий запрос и стадия.
enum Ep0State {
    Idle,
    /// Отдаём данные (IN data stage), осталось байт.
    Sending { data: [u8; 256], len: usize, pos: usize },
    /// Ждём данных (OUT data stage).
    Receiving,
}

static mut EP0: Ep0State = Ep0State::Idle;
/// Буферы приёма bulk-портов (кольцевые, по порту).
static mut PORT_RX: [[u8; 512]; desc::ACM_PORTS] = [[0; 512]; desc::ACM_PORTS];
static mut PORT_RX_LEN: [usize; desc::ACM_PORTS] = [0; desc::ACM_PORTS];

/// Сброс шины (USBRST): очистить состояние.
pub fn on_bus_reset() {
    unsafe {
        EP0 = Ep0State::Idle;
        for l in PORT_RX_LEN.iter_mut() {
            *l = 0;
        }
    }
}

/// Перечисление завершено (ENUMDONE): скорость готова.
pub fn on_enum_done(_speed: u32) {}

/// Разбор SETUP-пакета (8 байт).
pub fn on_setup(setup: &[u8; 8]) {
    let bm_req_type = setup[0];
    let request = setup[1];
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let _length = u16::from_le_bytes([setup[6], setup[7]]);

    // направление: бит7 = IN (хост читает)
    let is_in = bm_req_type & 0x80 != 0;
    // тип: 00 стандарт
    let recipient = bm_req_type & 0x1F;

    if recipient == 0 {
        // ---- стандартные запросы к устройству ----
        match (request, is_in) {
            (0x00, true) => {
                // GET_STATUS: устройство, remote wakeup off, bus powered
                send_data(&[0x00, 0x00]);
            }
            (0x05, false) => {
                // SET_ADDRESS
                dwc::set_address((value & 0x7F) as u32);
                dwc::ep0_send_status();
            }
            (0x06, true) => {
                // GET_DESCRIPTOR
                let dtype = (value >> 8) as u8;
                match dtype {
                    0x01 => {
                        let d = desc::device_descriptor(dwc::enum_speed() == 0);
                        send_data(&d);
                    }
                    0x02 => {
                        let c = desc::config_descriptor();
                        send_data(&c[..desc::CONFIG_TOTAL_LEN]);
                    }
                    0x03 => {
                        match desc::string_descriptor((value & 0xFF) as u8) {
                            Some(s) => send_data(&s),
                            None => dwc::ep0_stall(),
                        }
                    }
                    0x06 => {
                        let q = desc::qualifier_descriptor();
                        send_data(&q);
                    }
                    _ => dwc::ep0_stall(),
                }
            }
            (0x08, true) => {
                // GET_CONFIGURATION
                send_data(&[0x01]);
            }
            (0x09, false) => {
                // SET_CONFIGURATION: поднять bulk/INT эндпоинты
                for port in 0..desc::ACM_PORTS {
                    dwc::ep_configure(port + 1, true, false); // bulk IN
                    dwc::ep_configure(port + 1, false, false); // bulk OUT
                    dwc::ep_configure(port + 4, true, true); // notify INT IN
                }
                dwc::ep0_send_status();
                crate::console::putln("USB: сконфигурирован (PID 0x1C05, 3x CDC-ACM)");
            }
            (0x0A, true) => {
                // GET_INTERFACE
                send_data(&[0x00]);
            }
            (0x0B, false) => {
                // SET_INTERFACE
                dwc::ep0_send_status();
            }
            _ => dwc::ep0_stall(),
        }
        return;
    }

    if recipient == 1 {
        // ---- запросы к интерфейсу (CDC) ----
        match (request, is_in) {
            (0x00, true) => send_data(&[0x00, 0x00]), // GET_STATUS интерфейса
            _ => dwc::ep0_stall(),
        }
        let _ = index;
        return;
    }

    if recipient == 2 {
        // ---- запросы к эндпоинту ----
        match (request, is_in) {
            (0x00, true) => send_data(&[0x00, 0x00]),
            (0x01, false) => {
                // CLEAR_FEATURE: ENDPOINT_HALT
                dwc::ep0_send_status();
            }
            (0x03, false) => {
                // SET_FEATURE: ENDPOINT_HALT
                dwc::ep0_stall();
            }
            _ => dwc::ep0_stall(),
        }
        return;
    }

    dwc::ep0_stall();
}

/// Данные OUT в data-stage EP0 (SET_LINE_CODING и т.п.).
pub fn on_out_data(ep: usize, data: &[u8]) {
    if ep == 0 {
        // SET_LINE_CODING (7 байт: speed/stop/parity/width) — принимаем
        dwc::ep0_send_status();
        return;
    }
    // bulk-порт: аккумулировать и отдать протоколу DLOAD/AT
    let port = (ep - 1) / 1; // bulk OUT EPn ↔ порт n-1
    if port < desc::ACM_PORTS {
        unsafe {
            let p = &mut PORT_RX[port];
            let l = &mut PORT_RX_LEN[port];
            for &b in data {
                if *l < p.len() {
                    p[*l] = b;
                    *l += 1;
                }
            }
        }
    }
}

/// OUT-передача завершена: отдать накопленное протоколу.
pub fn on_out_complete(ep: usize) {
    if ep == 0 {
        return;
    }
    let port = ep - 1;
    if port < desc::ACM_PORTS {
        unsafe {
            let len = PORT_RX_LEN[port];
            if len > 0 {
                // приём любого bulk-порта кормит единый AT/DLOAD-обработчик
                // (оригинал открывает VCOM №1 = PCUI; здесь — надёжнее:
                // любой порт, ответ уходит в тот же порт)
                crate::usb::deliver_to_protocol(port, &PORT_RX[port][..len]);
                PORT_RX_LEN[port] = 0;
                // перевооружить приём
                dwc::ep_out_arm(ep, 512);
            }
        }
    }
}

/// IN-передача завершена (можно слать дальше).
pub fn on_in_complete(ep: usize) {
    unsafe {
        if ep == 0 {
            if let Ep0State::Sending { data, len, pos } = &mut EP0 {
                // продолжение data-stage, если ещё есть
                let mut d = [0u8; 64];
                let remaining = *len - *pos;
                let n = remaining.min(64);
                d[..n].copy_from_slice(&data[*pos..*pos + n]);
                if n > 0 {
                    dwc::ep_write(0, &d[..n]);
                    *pos += n;
                } else {
                    // data-stage окончен: status-этап хост сделает сам
                    EP0 = Ep0State::Idle;
                }
            }
        } else {
            // bulk IN свободен — можно писать дальше
            crate::usb::on_tx_complete(ep);
        }
    }
}

/// Начать отдачу данных на EP0 (data stage).
fn send_data(data: &[u8]) {
    unsafe {
        let mut buf = [0u8; 256];
        let n = data.len().min(256);
        buf[..n].copy_from_slice(&data[..n]);
        EP0 = Ep0State::Sending {
            data: buf,
            len: n,
            pos: 0,
        };
        // первый чанк
        let chunk = n.min(64);
        let mut d = [0u8; 64];
        d[..chunk].copy_from_slice(&data[..chunk]);
        dwc::ep_write(0, &d[..chunk]);
        if let Ep0State::Sending { pos, .. } = &mut EP0 {
            *pos = chunk;
        }
    }
}
