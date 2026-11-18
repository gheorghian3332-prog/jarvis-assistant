//! Minimal read-only WMI client (`IWbemServices::ExecQuery`). Used for two optional data sources:
//! drive health (`root\Microsoft\Windows\Storage`, readable without admin) and CPU temperature
//! published by a running LibreHardwareMonitor (`root\LibreHardwareMonitor`).

use windows::core::{BSTR, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY,
    WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE,
};

const RPC_C_AUTHN_WINNT: u32 = 10;
const RPC_C_AUTHZ_NONE: u32 = 0;
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106_u32 as i32;

#[derive(Debug, thiserror::Error)]
#[error("WMI: {0}")]
pub struct WmiError(pub String);

impl From<windows::core::Error> for WmiError {
    fn from(error: windows::core::Error) -> Self {
        Self(error.message())
    }
}

/// One result row: the requested properties, in the requested order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub numbers: Vec<Option<f64>>,
    pub strings: Vec<Option<String>>,
}

/// Runs `wql` in `namespace` and returns `number_props` and `string_props` of every row.
/// Blocks the calling thread; call it from a blocking worker.
pub fn query(
    namespace: &str,
    wql: &str,
    number_props: &[&str],
    string_props: &[&str],
) -> Result<Vec<Row>, WmiError> {
    // SAFETY: plain COM calls on this thread; COM is initialised before and released after.
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let initialised = init.is_ok();
        if !initialised && init.0 != RPC_E_CHANGED_MODE {
            return Err(WmiError(format!("CoInitializeEx failed: {init:?}")));
        }
        let result = query_inner(namespace, wql, number_props, string_props);
        if initialised {
            CoUninitialize();
        }
        result
    }
}

unsafe fn query_inner(
    namespace: &str,
    wql: &str,
    number_props: &[&str],
    string_props: &[&str],
) -> Result<Vec<Row>, WmiError> {
    let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
    let services = locator.ConnectServer(
        &BSTR::from(namespace),
        &BSTR::new(),
        &BSTR::new(),
        &BSTR::new(),
        0,
        &BSTR::new(),
        None,
    )?;
    CoSetProxyBlanket(
        &services,
        RPC_C_AUTHN_WINNT,
        RPC_C_AUTHZ_NONE,
        PCWSTR::null(),
        RPC_C_AUTHN_LEVEL_CALL,
        RPC_C_IMP_LEVEL_IMPERSONATE,
        None,
        EOAC_NONE,
    )?;
    let flags = WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0;
    let rows = services.ExecQuery(
        &BSTR::from("WQL"),
        &BSTR::from(wql),
        windows::Win32::System::Wmi::WBEM_GENERIC_FLAG_TYPE(flags),
        None,
    )?;
    let mut out = Vec::new();
    loop {
        let mut objects: [Option<IWbemClassObject>; 1] = [None];
        let mut returned = 0u32;
        let hr = rows.Next(WBEM_INFINITE, &mut objects, &mut returned);
        if hr.is_err() || returned == 0 {
            break;
        }
        let Some(object) = objects[0].take() else {
            break;
        };
        out.push(Row {
            numbers: number_props
                .iter()
                .map(|name| read_number(&object, name))
                .collect(),
            strings: string_props
                .iter()
                .map(|name| read_string(&object, name))
                .collect(),
        });
    }
    Ok(out)
}

unsafe fn read_variant(object: &IWbemClassObject, name: &str) -> Option<windows::core::VARIANT> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut value = windows::core::VARIANT::default();
    object
        .Get(PCWSTR(wide.as_ptr()), 0, &mut value, None, None)
        .ok()?;
    Some(value)
}

unsafe fn read_number(object: &IWbemClassObject, name: &str) -> Option<f64> {
    let value = read_variant(object, name)?;
    f64::try_from(&value).ok()
}

unsafe fn read_string(object: &IWbemClassObject, name: &str) -> Option<String> {
    let value = read_variant(object, name)?;
    BSTR::try_from(&value).ok().map(|s| s.to_string())
}

/// A temperature sensor as LibreHardwareMonitor publishes it.
#[derive(Debug, Clone, PartialEq)]
pub struct TemperatureSensor {
    pub identifier: String,
    pub name: String,
    pub celsius: f64,
}

/// Picks the CPU temperature: `CPU Package` / `Core (Tctl/Tdie)` when present, otherwise the
/// hottest CPU sensor. Sensors that are not on a CPU are ignored; `None` if there is none.
pub fn pick_cpu_temperature(sensors: &[TemperatureSensor]) -> Option<f32> {
    let cpu: Vec<&TemperatureSensor> = sensors
        .iter()
        .filter(|s| {
            let id = s.identifier.to_ascii_lowercase();
            (id.starts_with("/intelcpu/") || id.starts_with("/amdcpu/"))
                && s.celsius.is_finite()
                && s.celsius > 0.0
        })
        .collect();
    let preferred = cpu.iter().find(|s| {
        let name = s.name.to_ascii_lowercase();
        name.contains("package") || name.contains("tctl") || name.contains("tdie")
    });
    let chosen = match preferred {
        Some(sensor) => Some(sensor.celsius),
        None => cpu.iter().map(|s| s.celsius).reduce(f64::max),
    };
    chosen.map(|c| c as f32)
}

/// CPU temperature from LibreHardwareMonitor's WMI provider. `None` when it is not running.
pub fn libre_hardware_monitor_cpu_temp() -> Option<f32> {
    let rows = query(
        "root\\LibreHardwareMonitor",
        "SELECT Identifier, Name, Value FROM Sensor WHERE SensorType='Temperature'",
        &["Value"],
        &["Identifier", "Name"],
    )
    .ok()?;
    let sensors: Vec<TemperatureSensor> = rows
        .into_iter()
        .filter_map(|row| {
            Some(TemperatureSensor {
                identifier: row.strings.first()?.clone()?,
                name: row.strings.get(1)?.clone().unwrap_or_default(),
                celsius: (*row.numbers.first()?)?,
            })
        })
        .collect();
    pick_cpu_temperature(&sensors)
}

/// Health of the physical drives, without admin rights (`MSFT_PhysicalDisk.HealthStatus`).
pub fn drive_health() -> Option<Vec<crate::metrics::DriveHealth>> {
    let rows = query(
        "root\\Microsoft\\Windows\\Storage",
        "SELECT FriendlyName, HealthStatus FROM MSFT_PhysicalDisk",
        &["HealthStatus"],
        &["FriendlyName"],
    )
    .ok()?;
    if rows.is_empty() {
        return None;
    }
    Some(
        rows.into_iter()
            .map(|row| crate::metrics::DriveHealth {
                name: row
                    .strings
                    .first()
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| "disk".into()),
                status: health_name(row.numbers.first().copied().flatten()).to_string(),
            })
            .collect(),
    )
}

fn health_name(code: Option<f64>) -> &'static str {
    match code.map(|c| c as i64) {
        Some(0) => "Healthy",
        Some(1) => "Warning",
        Some(2) => "Unhealthy",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(id: &str, name: &str, celsius: f64) -> TemperatureSensor {
        TemperatureSensor {
            identifier: id.into(),
            name: name.into(),
            celsius,
        }
    }

    #[test]
    fn prefers_package_sensor() {
        let sensors = [
            sensor("/intelcpu/0/temperature/1", "Core #1", 60.0),
            sensor("/intelcpu/0/temperature/16", "CPU Package", 72.5),
            sensor("/nvidiagpu/0/temperature/0", "GPU Core", 90.0),
        ];
        assert_eq!(pick_cpu_temperature(&sensors), Some(72.5));
    }

    #[test]
    fn falls_back_to_hottest_core_and_ignores_gpu() {
        let sensors = [
            sensor("/amdcpu/0/temperature/2", "Core #1", 55.0),
            sensor("/amdcpu/0/temperature/3", "Core #2", 61.0),
            sensor("/gpu-nvidia/0/temperature/0", "GPU Core", 90.0),
        ];
        assert_eq!(pick_cpu_temperature(&sensors), Some(61.0));
        assert_eq!(
            pick_cpu_temperature(&[sensor("/nvidiagpu/0/temperature/0", "GPU Core", 90.0)]),
            None
        );
        assert_eq!(pick_cpu_temperature(&[]), None);
    }

    #[test]
    fn health_codes() {
        assert_eq!(health_name(Some(0.0)), "Healthy");
        assert_eq!(health_name(Some(2.0)), "Unhealthy");
        assert_eq!(health_name(Some(5.0)), "Unknown");
        assert_eq!(health_name(None), "Unknown");
    }
}
