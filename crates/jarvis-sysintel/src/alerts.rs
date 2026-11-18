//! Alert rules. Pure logic: it takes timestamps and values, so it is tested on synthetic series.
//!
//! * A rule fires once its condition has held for the rule duration without a break.
//! * Once active, a rule resets only after the value stays below `threshold - hysteresis` for
//!   `hysteresis_hold_s`; values inside the band keep it active.
//! * A rule fires at most once per cooldown window; a suppressed episode fires when the cooldown ends.
//! * At most `max_voice_per_hour` alerts are marked `speak`; quiet hours mute voice (the alert is
//!   still delivered so the window can show it).
//!
//! Nothing here touches applications: alerts are only messages.

use crate::config::{AlertConfig, ThresholdRule};
use crate::metrics::{AppCrashed, HungWindow};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    CpuLoad,
    GpuTemp,
    CpuTemp,
    RamLoad,
    DiskSpace,
    InternetDown,
    WindowHung,
    AppCrashed,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CpuLoad => "cpu_load",
            Self::GpuTemp => "gpu_temp",
            Self::CpuTemp => "cpu_temp",
            Self::RamLoad => "ram_load",
            Self::DiskSpace => "disk_space",
            Self::InternetDown => "internet_down",
            Self::WindowHung => "window_hung",
            Self::AppCrashed => "app_crashed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alert {
    pub id: String,
    pub kind: AlertKind,
    pub severity: Severity,
    pub message_ru: String,
    pub message_en: String,
    /// Whether the voice layer may say it (rate limit and quiet hours apply).
    pub speak: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskSample {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// Everything the rules look at in one evaluation. `None` means the sensor is unavailable.
#[derive(Debug, Clone, Copy)]
pub struct AlertInput<'a> {
    pub now_ms: u64,
    /// Minutes since local midnight, for quiet hours.
    pub local_minute: u16,
    pub cpu_percent: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub cpu_temp_c: Option<f32>,
    pub ram_percent: Option<f32>,
    pub system_disk: Option<DiskSample>,
    /// `Some(false)` when there is no link or the last probe failed.
    pub internet_up: Option<bool>,
    pub hung: &'a [HungWindow],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Beyond the threshold.
    Over,
    /// Not over the threshold, but not far enough below it to reset an active rule.
    Band,
    /// Below `threshold - hysteresis`.
    Clear,
    Unknown,
}

#[derive(Debug, Default)]
struct Episode {
    over_since: Option<u64>,
    below_since: Option<u64>,
    active: bool,
    fired: bool,
}

impl Episode {
    /// Advances the state machine; true when the rule may fire now (active, not yet fired).
    fn step(&mut self, now: u64, level: Level, duration_ms: u64, hold_ms: u64) -> bool {
        match level {
            Level::Over => {
                self.below_since = None;
                if !self.active {
                    let since = *self.over_since.get_or_insert(now);
                    if now.saturating_sub(since) >= duration_ms {
                        self.active = true;
                    }
                }
            }
            Level::Band | Level::Unknown => {
                self.below_since = None;
                if !self.active {
                    self.over_since = None;
                }
            }
            Level::Clear => {
                if self.active {
                    let since = *self.below_since.get_or_insert(now);
                    if now.saturating_sub(since) >= hold_ms {
                        *self = Self::default();
                    }
                } else {
                    self.over_since = None;
                }
            }
        }
        self.active && !self.fired
    }
}

fn threshold_level(value: Option<f32>, rule: &ThresholdRule, hysteresis: f32) -> Level {
    match value {
        None => Level::Unknown,
        Some(v) if v > rule.threshold => Level::Over,
        Some(v) if v >= rule.threshold - hysteresis => Level::Band,
        Some(_) => Level::Clear,
    }
}

/// Russian plural for "процент" after a number.
pub fn percent_ru(n: u32) -> &'static str {
    let last_two = n % 100;
    if (11..=14).contains(&last_two) {
        return "процентов";
    }
    match n % 10 {
        1 => "процент",
        2..=4 => "процента",
        _ => "процентов",
    }
}

#[derive(Debug, Default)]
pub struct AlertEngine {
    config: AlertConfig,
    cpu: Episode,
    gpu_temp: Episode,
    cpu_temp: Episode,
    ram: Episode,
    disk: Episode,
    internet: Episode,
    hung: HashMap<isize, Episode>,
    last_fired: HashMap<String, u64>,
    voice_times: VecDeque<u64>,
    last_disk_check: Option<u64>,
    counter: u64,
}

impl AlertEngine {
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub fn config(&self) -> &AlertConfig {
        &self.config
    }

    /// Replaces the settings; running episodes keep their state.
    pub fn set_config(&mut self, config: AlertConfig) {
        self.config = config;
    }

    /// Evaluates all rules. `top_ram_consumer` is only called when the RAM alert fires.
    pub fn evaluate(
        &mut self,
        input: &AlertInput<'_>,
        top_ram_consumer: &mut dyn FnMut() -> Option<String>,
    ) -> Vec<Alert> {
        let mut out = Vec::new();
        let cfg = self.config;
        let now = input.now_ms;
        let hold = cfg.hysteresis_hold_s * 1000;

        if cfg.cpu_load.enabled {
            let level = threshold_level(input.cpu_percent, &cfg.cpu_load, cfg.hysteresis);
            let ready = self
                .cpu
                .step(now, level, cfg.cpu_load.duration_s * 1000, hold);
            if ready && self.cooldown_ok("cpu_load", now) {
                self.cpu.fired = true;
                let n = input.cpu_percent.unwrap_or(cfg.cpu_load.threshold).round() as u32;
                out.push(self.make(
                    AlertKind::CpuLoad,
                    Severity::Warning,
                    format!("Внимание. Процессор загружен на {n} {}.", percent_ru(n)),
                    format!("Warning. CPU load is {n} percent."),
                    input,
                ));
            }
        }
        if cfg.gpu_temp.enabled {
            let level = threshold_level(input.gpu_temp_c, &cfg.gpu_temp, cfg.hysteresis);
            let ready = self
                .gpu_temp
                .step(now, level, cfg.gpu_temp.duration_s * 1000, hold);
            if ready && self.cooldown_ok("gpu_temp", now) {
                self.gpu_temp.fired = true;
                out.push(self.make(
                    AlertKind::GpuTemp,
                    Severity::Critical,
                    "Внимание. Обнаружена повышенная температура видеокарты.".into(),
                    "Warning. The graphics card temperature is high.".into(),
                    input,
                ));
            }
        }
        if cfg.cpu_temp.enabled {
            let level = threshold_level(input.cpu_temp_c, &cfg.cpu_temp, cfg.hysteresis);
            let ready = self
                .cpu_temp
                .step(now, level, cfg.cpu_temp.duration_s * 1000, hold);
            if ready && self.cooldown_ok("cpu_temp", now) {
                self.cpu_temp.fired = true;
                out.push(self.make(
                    AlertKind::CpuTemp,
                    Severity::Critical,
                    "Внимание. Обнаружена повышенная температура процессора.".into(),
                    "Warning. The processor temperature is high.".into(),
                    input,
                ));
            }
        }
        if cfg.ram_load.enabled {
            let level = threshold_level(input.ram_percent, &cfg.ram_load, cfg.hysteresis);
            let ready = self
                .ram
                .step(now, level, cfg.ram_load.duration_s * 1000, hold);
            if ready && self.cooldown_ok("ram_load", now) {
                self.ram.fired = true;
                let n = input.ram_percent.unwrap_or(cfg.ram_load.threshold).round() as u32;
                let consumer = top_ram_consumer();
                let (ru, en) = match consumer {
                    Some(name) => (
                        format!(
                            "Оперативная память загружена на {n} {}. Основной потребитель — {name}.",
                            percent_ru(n)
                        ),
                        format!("Memory usage is {n} percent. The biggest consumer is {name}."),
                    ),
                    None => (
                        format!("Оперативная память загружена на {n} {}.", percent_ru(n)),
                        format!("Memory usage is {n} percent."),
                    ),
                };
                out.push(self.make(AlertKind::RamLoad, Severity::Warning, ru, en, input));
            }
        }
        if cfg.disk_space.enabled {
            if let Some(alert) = self.check_disk(input) {
                out.push(alert);
            }
        }
        if cfg.internet_down.enabled {
            let level = match input.internet_up {
                None => Level::Unknown,
                Some(false) => Level::Over,
                Some(true) => Level::Clear,
            };
            let ready = self
                .internet
                .step(now, level, cfg.internet_down.duration_s * 1000, hold);
            if ready && self.cooldown_ok("internet_down", now) {
                self.internet.fired = true;
                out.push(self.make(
                    AlertKind::InternetDown,
                    Severity::Warning,
                    "Внимание. Интернет-соединение недоступно.".into(),
                    "Warning. The internet connection is unavailable.".into(),
                    input,
                ));
            }
        }
        if cfg.window_hung.enabled {
            if let Some(alert) = self.check_hung(input) {
                out.push(alert);
            }
        } else {
            self.hung.clear();
        }
        out
    }

    /// An application crash is reported at once; the cooldown is kept per executable.
    pub fn on_crash(
        &mut self,
        crash: &AppCrashed,
        now_ms: u64,
        local_minute: u16,
    ) -> Option<Alert> {
        if !self.config.app_crash.enabled {
            return None;
        }
        let key = format!("crash:{}", crash.name.to_lowercase());
        if !self.cooldown_ok(&key, now_ms) {
            return None;
        }
        self.last_fired.insert(key, now_ms);
        let input = AlertInput {
            now_ms,
            local_minute,
            cpu_percent: None,
            gpu_temp_c: None,
            cpu_temp_c: None,
            ram_percent: None,
            system_disk: None,
            internet_up: None,
            hung: &[],
        };
        let name = display_app_name(&crash.name);
        Some(self.make_unkeyed(
            AlertKind::AppCrashed,
            Severity::Warning,
            format!("Внимание. Приложение {name} неожиданно завершилось."),
            format!("Warning. The application {name} has closed unexpectedly."),
            &input,
        ))
    }

    fn check_disk(&mut self, input: &AlertInput<'_>) -> Option<Alert> {
        let cfg = self.config.disk_space;
        let now = input.now_ms;
        if let Some(last) = self.last_disk_check {
            if now.saturating_sub(last) < cfg.check_every_s * 1000 {
                return None;
            }
        }
        let disk = input.system_disk?;
        self.last_disk_check = Some(now);
        let free_gb = disk.free_bytes as f64 / 1_073_741_824.0;
        let free_pct = if disk.total_bytes == 0 {
            100.0
        } else {
            disk.free_bytes as f64 / disk.total_bytes as f64 * 100.0
        };
        let low =
            free_gb < f64::from(cfg.min_free_gb) || free_pct < f64::from(cfg.min_free_percent);
        // Checks are minutes apart, so the 30 s hold is satisfied by the next check already.
        let recovered = free_gb >= f64::from(cfg.min_free_gb)
            && free_pct >= f64::from(cfg.min_free_percent + self.config.hysteresis);
        let level = if low {
            Level::Over
        } else if recovered {
            Level::Clear
        } else {
            Level::Band
        };
        let ready = self.disk.step(now, level, 0, 0);
        if ready && self.cooldown_ok("disk_space", now) {
            self.disk.fired = true;
            let gb = free_gb.floor() as u64;
            return Some(self.make(
                AlertKind::DiskSpace,
                Severity::Warning,
                format!(
                    "Внимание. На системном диске осталось мало места: свободно {gb} {}.",
                    gigabytes_ru(gb)
                ),
                format!("Warning. The system drive is running out of space: {gb} gigabytes free."),
                input,
            ));
        }
        None
    }

    fn check_hung(&mut self, input: &AlertInput<'_>) -> Option<Alert> {
        let cfg = self.config;
        let now = input.now_ms;
        let hold = cfg.hysteresis_hold_s * 1000;
        let duration = cfg.window_hung.duration_s * 1000;
        // Windows that are no longer hung stay tracked until the hold time has passed.
        for (hwnd, episode) in &mut self.hung {
            if !input.hung.iter().any(|w| w.hwnd == *hwnd) {
                episode.step(now, Level::Clear, duration, hold);
            }
        }
        self.hung
            .retain(|_, e| e.active || e.over_since.is_some() || e.below_since.is_some());
        let mut candidate: Option<&HungWindow> = None;
        for window in input.hung {
            let episode = self.hung.entry(window.hwnd).or_default();
            if episode.step(now, Level::Over, duration, hold) && candidate.is_none() {
                candidate = Some(window);
            }
        }
        let window = candidate?;
        if !self.cooldown_ok("window_hung", now) {
            return None;
        }
        if let Some(episode) = self.hung.get_mut(&window.hwnd) {
            episode.fired = true;
        }
        let name = window
            .process
            .as_deref()
            .map(display_app_name)
            .unwrap_or_else(|| window.title.clone());
        Some(self.make(
            AlertKind::WindowHung,
            Severity::Warning,
            format!("Внимание. Приложение {name} не отвечает."),
            format!("Warning. The application {name} is not responding."),
            input,
        ))
    }

    fn cooldown_ok(&self, key: &str, now: u64) -> bool {
        self.last_fired
            .get(key)
            .is_none_or(|last| now.saturating_sub(*last) >= self.config.cooldown_s * 1000)
    }

    /// Builds an alert and records its cooldown under the kind name.
    fn make(
        &mut self,
        kind: AlertKind,
        severity: Severity,
        ru: String,
        en: String,
        input: &AlertInput<'_>,
    ) -> Alert {
        self.last_fired
            .insert(kind.as_str().to_string(), input.now_ms);
        self.make_unkeyed(kind, severity, ru, en, input)
    }

    fn make_unkeyed(
        &mut self,
        kind: AlertKind,
        severity: Severity,
        ru: String,
        en: String,
        input: &AlertInput<'_>,
    ) -> Alert {
        self.counter += 1;
        let speak = self.allow_voice(input.now_ms, input.local_minute);
        Alert {
            id: format!("{}-{}", kind.as_str(), self.counter),
            kind,
            severity,
            message_ru: ru,
            message_en: en,
            speak,
        }
    }

    fn allow_voice(&mut self, now: u64, local_minute: u16) -> bool {
        if self.config.quiet_hours.contains(local_minute) {
            return false;
        }
        while self
            .voice_times
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= 3_600_000)
        {
            self.voice_times.pop_front();
        }
        if self.voice_times.len() >= self.config.max_voice_per_hour as usize {
            return false;
        }
        self.voice_times.push_back(now);
        true
    }
}

fn gigabytes_ru(n: u64) -> &'static str {
    let last_two = n % 100;
    if (11..=14).contains(&last_two) {
        return "гигабайт";
    }
    match n % 10 {
        1 => "гигабайт",
        2..=4 => "гигабайта",
        _ => "гигабайт",
    }
}

/// `chrome.exe` becomes `chrome`; names that already read well are kept.
fn display_app_name(exe: &str) -> String {
    let len = exe.len();
    if len > 4 && exe.is_char_boundary(len - 4) && exe[len - 4..].eq_ignore_ascii_case(".exe") {
        exe[..len - 4].to_string()
    } else {
        exe.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{QuietHours, ThresholdRule};

    const S: u64 = 1000;

    fn input(now_s: u64) -> AlertInput<'static> {
        AlertInput {
            now_ms: now_s * S,
            local_minute: 12 * 60,
            cpu_percent: None,
            gpu_temp_c: None,
            cpu_temp_c: None,
            ram_percent: None,
            system_disk: None,
            internet_up: None,
            hung: &[],
        }
    }

    fn run(engine: &mut AlertEngine, i: AlertInput<'_>) -> Vec<Alert> {
        engine.evaluate(&i, &mut || Some("Google Chrome".to_string()))
    }

    fn cpu(now_s: u64, value: f32) -> AlertInput<'static> {
        AlertInput {
            cpu_percent: Some(value),
            ..input(now_s)
        }
    }

    #[test]
    fn cpu_alert_needs_sixty_continuous_seconds() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        for t in (0..60).step_by(5) {
            assert!(run(&mut engine, cpu(t, 95.0)).is_empty(), "t={t}");
        }
        let fired = run(&mut engine, cpu(60, 95.0));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].kind, AlertKind::CpuLoad);
        assert!(fired[0].speak);
        // One alert per episode.
        for t in (65..300).step_by(5) {
            assert!(run(&mut engine, cpu(t, 99.0)).is_empty());
        }
    }

    #[test]
    fn a_dip_below_the_threshold_restarts_the_duration() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        for t in (0..=50).step_by(5) {
            assert!(run(&mut engine, cpu(t, 95.0)).is_empty());
        }
        assert!(run(&mut engine, cpu(55, 80.0)).is_empty());
        for t in (60..=110).step_by(5) {
            assert!(run(&mut engine, cpu(t, 95.0)).is_empty(), "t={t}");
        }
        assert_eq!(run(&mut engine, cpu(120, 95.0)).len(), 1);
    }

    #[test]
    fn hysteresis_keeps_the_rule_active_inside_the_band() {
        let config = AlertConfig {
            cooldown_s: 0,
            ..AlertConfig::default()
        };
        let mut engine = AlertEngine::new(config);
        let mut t = 0;
        while t <= 60 {
            run(&mut engine, cpu(t, 95.0));
            t += 5;
        }
        // 87 is within 90 - 5..90: still the same episode, nothing resets.
        for _ in 0..30 {
            assert!(run(&mut engine, cpu(t, 87.0)).is_empty());
            t += 5;
        }
        for _ in 0..30 {
            assert!(
                run(&mut engine, cpu(t, 95.0)).is_empty(),
                "same episode must not fire twice"
            );
            t += 5;
        }
    }

    #[test]
    fn hysteresis_resets_after_thirty_seconds_below_the_band() {
        let config = AlertConfig {
            cooldown_s: 0,
            ..AlertConfig::default()
        };
        let mut engine = AlertEngine::new(config);
        let mut t = 0;
        while t <= 60 {
            run(&mut engine, cpu(t, 95.0));
            t += 5;
        }
        // 80 < 85 for only 25 s, then back up: not reset.
        for _ in 0..5 {
            run(&mut engine, cpu(t, 80.0));
            t += 5;
        }
        run(&mut engine, cpu(t, 95.0));
        t += 5;
        // 80 for a full 30 s: reset.
        for _ in 0..7 {
            run(&mut engine, cpu(t, 80.0));
            t += 5;
        }
        let mut fired = 0;
        for _ in 0..14 {
            fired += run(&mut engine, cpu(t, 95.0)).len();
            t += 5;
        }
        assert_eq!(fired, 1, "a new episode after the reset fires again");
    }

    #[test]
    fn cooldown_suppresses_then_releases_a_persistent_episode() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let mut fired_at = Vec::new();
        let mut t = 0;
        // Episode 1 fires at t=60, reset by 40 s of low load, episode 2 starts at once.
        while t <= 60 {
            if !run(&mut engine, cpu(t, 95.0)).is_empty() {
                fired_at.push(t);
            }
            t += 5;
        }
        for _ in 0..8 {
            run(&mut engine, cpu(t, 10.0));
            t += 5;
        }
        // Episode 2 is long enough at t ~ 165 but the cooldown lasts until 60 + 900 s.
        while t < 1200 {
            if !run(&mut engine, cpu(t, 95.0)).is_empty() {
                fired_at.push(t);
            }
            t += 5;
        }
        assert_eq!(fired_at.len(), 2, "{fired_at:?}");
        assert_eq!(fired_at[0], 60);
        assert!(fired_at[1] >= 60 + 900, "{fired_at:?}");
    }

    #[test]
    fn voice_is_limited_to_three_per_hour() {
        let config = AlertConfig {
            cooldown_s: 0,
            ..AlertConfig::default()
        };
        let mut engine = AlertEngine::new(config);
        let mut spoken = Vec::new();
        for i in 0..5u64 {
            let crash = AppCrashed {
                name: format!("app{i}.exe"),
                time_ms: 0,
            };
            let alert = engine.on_crash(&crash, i * 60 * S, 600).expect("alert");
            spoken.push(alert.speak);
        }
        assert_eq!(spoken, vec![true, true, true, false, false]);
        // An hour after the first one, voice is allowed again.
        let crash = AppCrashed {
            name: "late.exe".into(),
            time_ms: 0,
        };
        let alert = engine.on_crash(&crash, 3_700 * S, 600).expect("alert");
        assert!(alert.speak);
    }

    #[test]
    fn quiet_hours_mute_voice_but_not_the_alert() {
        let config = AlertConfig {
            quiet_hours: QuietHours {
                enabled: true,
                start_minute: 22 * 60,
                end_minute: 7 * 60,
            },
            ..AlertConfig::default()
        };
        let mut engine = AlertEngine::new(config);
        let crash = AppCrashed {
            name: "game.exe".into(),
            time_ms: 0,
        };
        let night = engine.on_crash(&crash, 0, 23 * 60).expect("alert");
        assert!(!night.speak);
        let other = AppCrashed {
            name: "other.exe".into(),
            time_ms: 0,
        };
        let day = engine.on_crash(&other, 10 * S, 12 * 60).expect("alert");
        assert!(day.speak, "muted alerts do not use the hourly voice budget");
    }

    #[test]
    fn crash_cooldown_is_per_application() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let a = AppCrashed {
            name: "a.exe".into(),
            time_ms: 0,
        };
        let b = AppCrashed {
            name: "b.exe".into(),
            time_ms: 0,
        };
        assert!(engine.on_crash(&a, 0, 0).is_some());
        assert!(engine.on_crash(&a, 100 * S, 0).is_none());
        assert!(engine.on_crash(&b, 100 * S, 0).is_some());
        assert!(engine.on_crash(&a, 901 * S, 0).is_some());
    }

    #[test]
    fn gpu_temperature_message_follows_the_spec() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let hot = |t: u64| AlertInput {
            gpu_temp_c: Some(90.0),
            ..input(t)
        };
        assert!(run(&mut engine, hot(0)).is_empty());
        assert!(run(&mut engine, hot(29)).is_empty());
        let alerts = run(&mut engine, hot(30));
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts[0].message_ru,
            "Внимание. Обнаружена повышенная температура видеокарты."
        );
        assert_eq!(alerts[0].severity, Severity::Critical);
    }

    #[test]
    fn ram_alert_names_the_main_consumer() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let mut calls = 0;
        let mut consumer = || {
            calls += 1;
            Some("Google Chrome".to_string())
        };
        let mk = |t: u64| AlertInput {
            ram_percent: Some(94.0),
            ..input(t)
        };
        assert!(engine.evaluate(&mk(0), &mut consumer).is_empty());
        assert!(engine.evaluate(&mk(20), &mut consumer).is_empty());
        let alerts = engine.evaluate(&mk(30), &mut consumer);
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts[0].message_ru,
            "Оперативная память загружена на 94 процента. Основной потребитель — Google Chrome."
        );
        assert_eq!(
            calls, 1,
            "the consumer is looked up only when the alert fires"
        );
    }

    #[test]
    fn missing_sensors_never_alert() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        for t in (0..600).step_by(5) {
            assert!(run(&mut engine, input(t)).is_empty());
        }
    }

    #[test]
    fn disk_rule_checks_every_ten_minutes() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let low = |t: u64| AlertInput {
            system_disk: Some(DiskSample {
                free_bytes: 8 << 30,
                total_bytes: 500 << 30,
            }),
            ..input(t)
        };
        let alerts = run(&mut engine, low(0));
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, AlertKind::DiskSpace);
        assert!(alerts[0].message_ru.contains("8 гигабайт"));
        // Same episode: no repeat. A healthy disk 10 minutes later resets it.
        assert!(run(&mut engine, low(300)).is_empty());
        let healthy = AlertInput {
            system_disk: Some(DiskSample {
                free_bytes: 200 << 30,
                total_bytes: 500 << 30,
            }),
            ..input(600)
        };
        assert!(run(&mut engine, healthy).is_empty());
        // Percent rule: 20 GB of 1 TB is below 5%.
        let tiny = AlertInput {
            system_disk: Some(DiskSample {
                free_bytes: 20 << 30,
                total_bytes: 1000 << 30,
            }),
            ..input(2000)
        };
        assert_eq!(run(&mut engine, tiny).len(), 1);
    }

    #[test]
    fn internet_outage_needs_twenty_seconds() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let down = |t: u64| AlertInput {
            internet_up: Some(false),
            ..input(t)
        };
        assert!(run(&mut engine, down(0)).is_empty());
        assert!(run(&mut engine, down(15)).is_empty());
        let alerts = run(&mut engine, down(20));
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, AlertKind::InternetDown);
        // A blip shorter than 20 s never fires.
        let mut engine = AlertEngine::new(AlertConfig::default());
        assert!(run(&mut engine, down(0)).is_empty());
        let up = AlertInput {
            internet_up: Some(true),
            ..input(10)
        };
        assert!(run(&mut engine, up).is_empty());
        assert!(run(&mut engine, down(15)).is_empty());
        assert!(run(&mut engine, down(30)).is_empty());
    }

    #[test]
    fn hung_window_needs_fifteen_seconds() {
        let mut engine = AlertEngine::new(AlertConfig::default());
        let windows = [HungWindow {
            hwnd: 42,
            pid: 7,
            process: Some("notepad.exe".into()),
            title: "Untitled".into(),
        }];
        let mk = |t: u64| AlertInput {
            hung: &windows,
            ..input(t)
        };
        assert!(run(&mut engine, mk(0)).is_empty());
        assert!(run(&mut engine, mk(10)).is_empty());
        let alerts = run(&mut engine, mk(15));
        assert_eq!(alerts.len(), 1);
        assert_eq!(
            alerts[0].message_ru,
            "Внимание. Приложение notepad не отвечает."
        );
        assert!(run(&mut engine, mk(20)).is_empty());
    }

    #[test]
    fn disabled_rule_is_silent() {
        let config = AlertConfig {
            gpu_temp: ThresholdRule {
                enabled: false,
                ..ThresholdRule::default()
            },
            ..AlertConfig::default()
        };
        let mut engine = AlertEngine::new(config);
        let hot = |t: u64| AlertInput {
            gpu_temp_c: Some(99.0),
            ..input(t)
        };
        run(&mut engine, hot(0));
        assert!(run(&mut engine, hot(60)).is_empty());
    }

    #[test]
    fn plural_forms() {
        assert_eq!(percent_ru(1), "процент");
        assert_eq!(percent_ru(94), "процента");
        assert_eq!(percent_ru(95), "процентов");
        assert_eq!(percent_ru(11), "процентов");
        assert_eq!(percent_ru(101), "процент");
    }
}
