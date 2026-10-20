//! version.rs — версии и идентификаторы загрузчика.
//!
//! ОРИГИНАЛ (docs/04 §3.6, литералы образа):
//!   * DLOAD-версия:        "2.0"       (getDloadVersion → DAT_30076298)
//!   * версия ПО:           из NV (BSP_OM_GetDloadVer), дефолт "21.420.99.07.00"
//!   * bootrom-версия:      BSP_DLOAD_GetBootromVer
//!   * authority-версия:    "1000"      (getAuthorityVersion)
//!   * authority ID:        IMEI из NV + ",0" (getAuthorityId);
//!                          при пустой NV — заглушка
//!   * DLOAD ID:            имя продукта (CH1E3272SM), дефолт "H69DUSAVA"
//! Здесь NV-часть деградирует до дефолтов (NV-подсистема вне объёма порта —
//! как и в самом репозитории-реконструкции, docs/06 §8).

/// DLOAD-версия протокола.
pub const DLOAD_VER: &[u8] = b"2.0";
/// Версия ПО по умолчанию (NV пуст/недоступен).
pub const SOFTWARE_VER_DEFAULT: &[u8] = b"21.420.99.07.00";
/// Версия авторизации.
pub const AUTHORITY_VER: &[u8] = b"1000";
/// DLOAD ID по умолчанию (плата H69DUSAVA).
pub const DLOAD_ID_DEFAULT: &[u8] = b"H69DUSAVA";
/// Версия bootrom (для AT^BOOTROMVER? — не извлечена из образа; как в
/// оригинале поле заполняется на живом устройстве, здесь константа).
pub const BOOTROM_VER: &[u8] = b"1.0";

/// C-строка версии ПО (с заглушкой NV, как BSP_OM_GetDloadVer при ошибке).
pub fn software_ver() -> &'static [u8] {
    SOFTWARE_VER_DEFAULT
}

/// DLOAD ID: имя продукта из таблицы (E3272s-153 → "CH1E3272SM"),
/// при отсутствии — "H69DUSAVA" (getDloadID @0x30075634).
pub fn dload_id() -> &'static [u8] {
    crate::product::dload_name().unwrap_or(DLOAD_ID_DEFAULT)
}

/// Authority ID: как getAuthorityId — IMEI + ",0"; без NV возвращаем
/// "0,0" (оригинал при ошибке NV отдаёт ERROR через AT-обработчик).
pub fn authority_id(buf: &mut [u8]) -> usize {
    // IMEI недоступен без NV-слоя — деградация (см. модуль-док)
    let s = b"0,0";
    buf[..s.len()].copy_from_slice(s);
    buf[s.len()] = 0;
    s.len()
}
