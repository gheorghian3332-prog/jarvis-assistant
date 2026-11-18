//! Short spoken answers about the machine (`system.status`), built from real metrics only.
//! A value the sensor does not provide is said to be unavailable, never guessed.

use crate::alerts::{percent_ru, Alert};
use crate::metrics::{GpuMetrics, Metrics, ProcessUsage};

/// What the user asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    /// "какая температура видеокарты"
    GpuTemp,
    /// "загрузка видеокарты"
    Gpu,
    /// "сколько свободно памяти"
    Memory,
    /// "загрузка процессора"
    Cpu,
    /// "что тормозит компьютер"
    Slowdown,
    /// "проверь систему"
    Overview,
}

pub const TOPIC_NAMES: [&str; 6] = ["gpu_temp", "gpu", "memory", "cpu", "slowdown", "overview"];

impl Topic {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "gpu_temp" => Self::GpuTemp,
            "gpu" => Self::Gpu,
            "memory" => Self::Memory,
            "cpu" => Self::Cpu,
            "slowdown" => Self::Slowdown,
            "overview" => Self::Overview,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GpuTemp => "gpu_temp",
            Self::Gpu => "gpu",
            Self::Memory => "memory",
            Self::Cpu => "cpu",
            Self::Slowdown => "slowdown",
            Self::Overview => "overview",
        }
    }

    /// The topic needs the process rankings (they are sampled on demand).
    pub fn needs_processes(self) -> bool {
        matches!(self, Self::Slowdown | Self::Overview)
    }
}

/// An answer in both languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub ru: String,
    pub en: String,
}

/// Plural of a Russian noun after a number: `forms` = (1, 2-4, 5+).
fn plural(n: u32, forms: (&'static str, &'static str, &'static str)) -> &'static str {
    let last_two = n % 100;
    if (11..=14).contains(&last_two) {
        return forms.2;
    }
    match n % 10 {
        1 => forms.0,
        2..=4 => forms.1,
        _ => forms.2,
    }
}

fn degrees_ru(n: u32) -> String {
    format!("{n} {}", plural(n, ("градус", "градуса", "градусов")))
}

fn percent_text_ru(n: u32) -> String {
    format!("{n} {}", percent_ru(n))
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// "9,3 гигабайта" / "8 гигабайт" (a fraction takes the genitive singular).
fn gigabytes_ru(bytes: u64) -> String {
    let value = (gib(bytes) * 10.0).round() / 10.0;
    if (value - value.round()).abs() < 0.05 {
        let whole = value.round() as u32;
        format!(
            "{whole} {}",
            plural(whole, ("гигабайт", "гигабайта", "гигабайт"))
        )
    } else {
        format!("{value:.1} гигабайта").replace('.', ",")
    }
}

fn gigabytes_en(bytes: u64) -> String {
    format!("{:.1} GB", gib(bytes))
}

fn gpu_name(gpu: &GpuMetrics) -> String {
    gpu.name.clone().unwrap_or_else(|| "GPU".into())
}

fn gpu_temp(metrics: &Metrics) -> Report {
    match metrics
        .gpu
        .as_ref()
        .and_then(|g| g.temperature_c.map(|t| (g, t)))
    {
        Some((gpu, temp)) => Report {
            ru: format!(
                "Температура видеокарты {} — {}.",
                gpu_name(gpu),
                degrees_ru(temp)
            ),
            en: format!("The {} is at {temp} degrees Celsius.", gpu_name(gpu)),
        },
        None => Report {
            ru: "Температуру видеокарты сейчас прочитать не удалось.".into(),
            en: "I cannot read the GPU temperature right now.".into(),
        },
    }
}

fn gpu_load(metrics: &Metrics) -> Report {
    let Some(gpu) = metrics.gpu.as_ref() else {
        return Report {
            ru: "Данных о видеокарте сейчас нет.".into(),
            en: "There is no GPU data right now.".into(),
        };
    };
    let mut ru = Vec::new();
    let mut en = Vec::new();
    if let Some(load) = gpu.utilization_percent {
        ru.push(format!("загружена на {}", percent_text_ru(load)));
        en.push(format!("{load}% busy"));
    }
    if let (Some(used), Some(total)) = (gpu.vram_used_bytes, gpu.vram_total_bytes) {
        ru.push(format!(
            "видеопамять {} из {}",
            gigabytes_ru(used),
            gigabytes_ru(total)
        ));
        en.push(format!(
            "video memory {} of {}",
            gigabytes_en(used),
            gigabytes_en(total)
        ));
    }
    if let Some(temp) = gpu.temperature_c {
        ru.push(degrees_ru(temp));
        en.push(format!("{temp} degrees"));
    }
    if ru.is_empty() {
        return Report {
            ru: "Видеокарта не отдаёт показания.".into(),
            en: "The GPU reports no readings.".into(),
        };
    }
    Report {
        ru: format!("Видеокарта {}: {}.", gpu_name(gpu), ru.join(", ")),
        en: format!("The {}: {}.", gpu_name(gpu), en.join(", ")),
    }
}

fn memory(metrics: &Metrics) -> Report {
    let ram = &metrics.ram;
    if ram.total_bytes == 0 {
        return Report {
            ru: "Данных о памяти пока нет.".into(),
            en: "There is no memory data yet.".into(),
        };
    }
    let used = ram.used_percent().unwrap_or(0.0).round() as u32;
    Report {
        ru: format!(
            "Свободно {} из {}, занято {}.",
            gigabytes_ru(ram.available_bytes),
            gigabytes_ru(ram.total_bytes),
            percent_text_ru(used)
        ),
        en: format!(
            "{} free of {}, {used}% in use.",
            gigabytes_en(ram.available_bytes),
            gigabytes_en(ram.total_bytes)
        ),
    }
}

fn cpu(metrics: &Metrics) -> Report {
    let Some(load) = metrics.cpu.usage_percent else {
        return Report {
            ru: "Данных о процессоре пока нет.".into(),
            en: "There is no CPU data yet.".into(),
        };
    };
    let load = load.round() as u32;
    let mut ru = format!("Процессор загружен на {}", percent_text_ru(load));
    let mut en = format!("The CPU is {load}% busy");
    if let Some(mhz) = metrics.cpu.frequency_mhz {
        ru.push_str(&format!(", частота {mhz} мегагерц"));
        en.push_str(&format!(", running at {mhz} MHz"));
    }
    if let Some(temp) = metrics.cpu.temperature_c {
        let temp = temp.round() as u32;
        ru.push_str(&format!(", {}", degrees_ru(temp)));
        en.push_str(&format!(", {temp} degrees"));
    }
    Report {
        ru: ru + ".",
        en: en + ".",
    }
}

fn first(list: &[ProcessUsage]) -> Option<&ProcessUsage> {
    list.first()
}

/// The biggest problem on the machine right now, as words.
fn slowdown(metrics: &Metrics, top_cpu: &[ProcessUsage], top_ram: &[ProcessUsage]) -> Report {
    let mut ru = Vec::new();
    let mut en = Vec::new();
    if let Some(load) = metrics.cpu.usage_percent.filter(|l| *l >= 80.0) {
        let load = load.round() as u32;
        match first(top_cpu) {
            Some(p) => {
                ru.push(format!(
                    "процессор загружен на {}, больше всего берёт {}",
                    percent_text_ru(load),
                    p.display_name
                ));
                en.push(format!(
                    "the CPU is {load}% busy, mostly {}",
                    p.display_name
                ));
            }
            None => {
                ru.push(format!("процессор загружен на {}", percent_text_ru(load)));
                en.push(format!("the CPU is {load}% busy"));
            }
        }
    }
    if let Some(used) = metrics.ram.used_percent().filter(|u| *u >= 85.0) {
        let used = used.round() as u32;
        match first(top_ram) {
            Some(p) => {
                ru.push(format!(
                    "память занята на {}, больше всего у {}",
                    percent_text_ru(used),
                    p.display_name
                ));
                en.push(format!(
                    "memory is {used}% used, mostly by {}",
                    p.display_name
                ));
            }
            None => {
                ru.push(format!("память занята на {}", percent_text_ru(used)));
                en.push(format!("memory is {used}% used"));
            }
        }
    }
    if let Some(gpu) = &metrics.gpu {
        if let Some(load) = gpu.utilization_percent.filter(|l| *l >= 90) {
            ru.push(format!("видеокарта загружена на {}", percent_text_ru(load)));
            en.push(format!("the GPU is {load}% busy"));
        }
        if let Some(temp) = gpu.temperature_c.filter(|t| *t >= 83) {
            ru.push(format!("видеокарта горячая, {}", degrees_ru(temp)));
            en.push(format!("the GPU is hot at {temp} degrees"));
        }
    }
    if let Some(volume) = metrics
        .storage
        .volumes
        .iter()
        .find(|v| v.is_system && v.free_percent().is_some_and(|f| f < 5.0))
    {
        ru.push(format!("на диске {} почти нет места", volume.mount));
        en.push(format!("drive {} is almost full", volume.mount));
    }
    if let Some(window) = metrics.hung_windows.first() {
        ru.push(format!("не отвечает окно «{}»", window.title));
        en.push(format!("the window \"{}\" is not responding", window.title));
    }
    if metrics.network.internet_reachable == Some(false) {
        ru.push("нет связи с интернетом".into());
        en.push("there is no internet connection".into());
    }
    if ru.is_empty() {
        let load = metrics.cpu.usage_percent.map(|l| l.round() as u32);
        let used = metrics.ram.used_percent().map(|u| u.round() as u32);
        return Report {
            ru: match (load, used) {
                (Some(l), Some(u)) => format!(
                    "Ничего подозрительного: процессор {}, память {}.",
                    percent_text_ru(l),
                    percent_text_ru(u)
                ),
                _ => "Ничего подозрительного не вижу.".into(),
            },
            en: match (load, used) {
                (Some(l), Some(u)) => {
                    format!("Nothing stands out: CPU {l}%, memory {u}%.")
                }
                _ => "I see nothing suspicious.".into(),
            },
        };
    }
    Report {
        ru: format!("Вижу так: {}.", ru.join("; ")),
        en: format!("Here is what I see: {}.", en.join("; ")),
    }
}

fn overview(
    metrics: &Metrics,
    top_cpu: &[ProcessUsage],
    top_ram: &[ProcessUsage],
    alerts: &[Alert],
) -> Report {
    let mut ru = vec![cpu_short_ru(metrics), memory_short_ru(metrics)];
    let mut en = vec![cpu_short_en(metrics), memory_short_en(metrics)];
    if let Some(gpu) = &metrics.gpu {
        if let Some(temp) = gpu.temperature_c {
            ru.push(format!("видеокарта {}", degrees_ru(temp)));
            en.push(format!("GPU {temp} degrees"));
        }
    }
    ru.retain(|part| !part.is_empty());
    en.retain(|part| !part.is_empty());
    let problems = slowdown(metrics, top_cpu, top_ram);
    let calm = !problems.ru.starts_with("Вижу так");
    let tail_ru = match alerts.first() {
        Some(alert) => format!("Последнее предупреждение: {}", alert.message_ru),
        None => "Предупреждений нет.".into(),
    };
    let tail_en = match alerts.first() {
        Some(alert) => format!("Latest alert: {}", alert.message_en),
        None => "No alerts.".into(),
    };
    Report {
        ru: format!(
            "{}. {} {}",
            capitalize(&ru.join(", ")),
            if calm {
                "Всё в порядке."
            } else {
                problems.ru.as_str()
            },
            tail_ru
        ),
        en: format!(
            "{}. {} {}",
            capitalize(&en.join(", ")),
            if calm {
                "Everything looks fine."
            } else {
                problems.en.as_str()
            },
            tail_en
        ),
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn cpu_short_ru(m: &Metrics) -> String {
    m.cpu
        .usage_percent
        .map(|l| format!("процессор {}", percent_text_ru(l.round() as u32)))
        .unwrap_or_default()
}

fn cpu_short_en(m: &Metrics) -> String {
    m.cpu
        .usage_percent
        .map(|l| format!("CPU {}%", l.round() as u32))
        .unwrap_or_default()
}

fn memory_short_ru(m: &Metrics) -> String {
    m.ram
        .used_percent()
        .map(|u| format!("память {}", percent_text_ru(u.round() as u32)))
        .unwrap_or_default()
}

fn memory_short_en(m: &Metrics) -> String {
    m.ram
        .used_percent()
        .map(|u| format!("memory {}%", u.round() as u32))
        .unwrap_or_default()
}

/// The answer for `topic`. `top_cpu` / `top_ram` are only read by the topics that need them.
pub fn answer(
    topic: Topic,
    metrics: &Metrics,
    top_cpu: &[ProcessUsage],
    top_ram: &[ProcessUsage],
    alerts: &[Alert],
) -> Report {
    match topic {
        Topic::GpuTemp => gpu_temp(metrics),
        Topic::Gpu => gpu_load(metrics),
        Topic::Memory => memory(metrics),
        Topic::Cpu => cpu(metrics),
        Topic::Slowdown => slowdown(metrics, top_cpu, top_ram),
        Topic::Overview => overview(metrics, top_cpu, top_ram, alerts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::{AlertKind, Severity};
    use crate::metrics::{CpuMetrics, HungWindow, RamMetrics, VolumeMetrics};

    const GIB: u64 = 1024 * 1024 * 1024;

    fn fixture() -> Metrics {
        Metrics {
            cpu: CpuMetrics {
                usage_percent: Some(23.4),
                per_core_percent: vec![],
                frequency_mhz: Some(3600),
                temperature_c: None,
            },
            gpu: Some(GpuMetrics {
                name: Some("NVIDIA GeForce GTX 1660".into()),
                utilization_percent: Some(12),
                temperature_c: Some(74),
                vram_used_bytes: Some(2 * GIB),
                vram_total_bytes: Some(6 * GIB),
                ..GpuMetrics::default()
            }),
            ram: RamMetrics {
                used_bytes: 7 * GIB,
                total_bytes: 16 * GIB,
                available_bytes: 9 * GIB + GIB / 4,
            },
            ..Metrics::default()
        }
    }

    fn process(name: &str) -> ProcessUsage {
        ProcessUsage {
            exe: format!("{name}.exe"),
            display_name: name.into(),
            instances: 1,
            memory_mb: 1000,
            cpu_percent: Some(50.0),
        }
    }

    #[test]
    fn the_gpu_temperature_is_the_real_number() {
        let report = answer(Topic::GpuTemp, &fixture(), &[], &[], &[]);
        assert_eq!(
            report.ru,
            "Температура видеокарты NVIDIA GeForce GTX 1660 — 74 градуса."
        );
        assert!(report.en.contains("74 degrees"));
    }

    #[test]
    fn a_missing_gpu_reading_is_said_to_be_unavailable() {
        let mut metrics = fixture();
        metrics.gpu = None;
        let report = answer(Topic::GpuTemp, &metrics, &[], &[], &[]);
        assert!(report.ru.contains("не удалось"));
        assert!(!report.ru.chars().any(|c| c.is_ascii_digit()));
        metrics.gpu = Some(GpuMetrics::default());
        assert!(answer(Topic::GpuTemp, &metrics, &[], &[], &[])
            .ru
            .contains("не удалось"));
        assert!(answer(Topic::Gpu, &metrics, &[], &[], &[])
            .ru
            .contains("не отдаёт"));
    }

    #[test]
    fn memory_reports_free_and_total() {
        let report = answer(Topic::Memory, &fixture(), &[], &[], &[]);
        assert_eq!(
            report.ru,
            "Свободно 9,3 гигабайта из 16 гигабайт, занято 44 процента."
        );
        assert!(report.en.starts_with("9.2 GB free of 16.0 GB"));
        assert!(answer(Topic::Memory, &Metrics::default(), &[], &[], &[])
            .ru
            .contains("нет"));
    }

    #[test]
    fn cpu_reports_load_and_frequency() {
        let report = answer(Topic::Cpu, &fixture(), &[], &[], &[]);
        assert_eq!(
            report.ru,
            "Процессор загружен на 23 процента, частота 3600 мегагерц."
        );
    }

    #[test]
    fn slowdown_names_the_heaviest_process() {
        let mut metrics = fixture();
        metrics.cpu.usage_percent = Some(96.0);
        metrics.ram.used_bytes = 15 * GIB;
        let report = answer(
            Topic::Slowdown,
            &metrics,
            &[process("Chrome")],
            &[process("Docker")],
            &[],
        );
        assert!(report
            .ru
            .contains("процессор загружен на 96 процентов, больше всего берёт Chrome"));
        assert!(report.ru.contains("больше всего у Docker"));
        let calm = answer(Topic::Slowdown, &fixture(), &[], &[], &[]);
        assert!(calm.ru.starts_with("Ничего подозрительного"));
    }

    #[test]
    fn slowdown_sees_hung_windows_and_full_disks() {
        let mut metrics = fixture();
        metrics.hung_windows = vec![HungWindow {
            hwnd: 1,
            pid: 2,
            process: None,
            title: "Редактор".into(),
        }];
        metrics.storage.volumes = vec![VolumeMetrics {
            mount: "C:\\".into(),
            is_system: true,
            total_bytes: 100 * GIB,
            free_bytes: 2 * GIB,
            ..VolumeMetrics::default()
        }];
        let report = answer(Topic::Slowdown, &metrics, &[], &[], &[]);
        assert!(report.ru.contains("не отвечает окно «Редактор»"));
        assert!(report.ru.contains("почти нет места"));
    }

    #[test]
    fn the_overview_sums_up_and_mentions_alerts() {
        let alert = Alert {
            id: "a-1".into(),
            kind: AlertKind::GpuTemp,
            severity: Severity::Warning,
            message_ru: "Видеокарта перегрета".into(),
            message_en: "The GPU is overheating".into(),
            speak: true,
        };
        let quiet = answer(Topic::Overview, &fixture(), &[], &[], &[]);
        assert!(quiet
            .ru
            .starts_with("Процессор 23 процента, память 44 процента, видеокарта 74 градуса."));
        assert!(quiet.ru.contains("Всё в порядке") && quiet.ru.contains("Предупреждений нет"));
        let loud = answer(Topic::Overview, &fixture(), &[], &[], &[alert]);
        assert!(loud
            .ru
            .contains("Последнее предупреждение: Видеокарта перегрета"));
    }

    #[test]
    fn topics_round_trip() {
        for name in TOPIC_NAMES {
            assert_eq!(Topic::parse(name).map(Topic::as_str), Some(name));
        }
        assert_eq!(Topic::parse("weather"), None);
        assert!(Topic::Slowdown.needs_processes() && !Topic::Cpu.needs_processes());
    }

    #[test]
    fn plurals_follow_russian_rules() {
        assert_eq!(degrees_ru(21), "21 градус");
        assert_eq!(degrees_ru(74), "74 градуса");
        assert_eq!(degrees_ru(85), "85 градусов");
        assert_eq!(degrees_ru(11), "11 градусов");
        assert_eq!(gigabytes_ru(8 * GIB), "8 гигабайт");
        assert_eq!(gigabytes_ru(2 * GIB), "2 гигабайта");
        assert_eq!(gigabytes_ru(GIB), "1 гигабайт");
    }
}
