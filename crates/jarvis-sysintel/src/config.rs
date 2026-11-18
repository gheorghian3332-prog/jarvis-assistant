//! Polling rates (ARCHITECTURE.md, "Частоты опроса") and alert settings.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Who is looking at the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Tray only, no UI subscriber: sample just enough for the alert rules.
    Tray,
    /// The window is open and shows live values.
    Window,
}

/// How often each sensor group is read in a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rates {
    pub cpu_ram: Duration,
    pub gpu: Duration,
    /// Network counters and disk activity.
    pub net_io: Duration,
    pub rtt: Duration,
    pub hung_windows: Duration,
    /// Volume free space (and drive health).
    pub volumes: Duration,
}

impl Rates {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Tray => Self {
                cpu_ram: Duration::from_secs(5),
                gpu: Duration::from_secs(10),
                net_io: Duration::from_secs(10),
                rtt: Duration::from_secs(60),
                hung_windows: Duration::from_secs(10),
                volumes: Duration::from_secs(600),
            },
            Mode::Window => Self {
                cpu_ram: Duration::from_secs(1),
                gpu: Duration::from_secs(1),
                net_io: Duration::from_secs(1),
                rtt: Duration::from_secs(10),
                hung_windows: Duration::from_secs(2),
                volumes: Duration::from_secs(30),
            },
        }
    }
}

/// While the connection is considered down, probe this often so a 20 s outage is noticed.
pub const RTT_RETRY_WHILE_DOWN: Duration = Duration::from_secs(5);
/// While a hung window is known, look again this often to measure how long it hangs.
pub const HUNG_RETRY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SysIntelConfig {
    /// Host and port for the TCP-connect latency probe.
    pub rtt_host: String,
    pub alerts: AlertConfig,
}

impl Default for SysIntelConfig {
    fn default() -> Self {
        Self {
            rtt_host: "1.1.1.1:443".into(),
            alerts: AlertConfig::default(),
        }
    }
}

/// A threshold rule: fires after the value stays above `threshold` for `duration_s`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThresholdRule {
    pub enabled: bool,
    pub threshold: f32,
    pub duration_s: u64,
}

/// A rule without a numeric threshold (it only has a duration).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DurationRule {
    pub enabled: bool,
    pub duration_s: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskRule {
    pub enabled: bool,
    /// Alert when free space is below this many gigabytes...
    pub min_free_gb: f32,
    /// ...or below this share of the volume.
    pub min_free_percent: f32,
    pub check_every_s: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHours {
    pub enabled: bool,
    /// Minutes since local midnight. The range may wrap over midnight (22:00 to 07:00).
    pub start_minute: u16,
    pub end_minute: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertConfig {
    pub cpu_load: ThresholdRule,
    pub gpu_temp: ThresholdRule,
    pub cpu_temp: ThresholdRule,
    pub ram_load: ThresholdRule,
    pub disk_space: DiskRule,
    pub internet_down: DurationRule,
    pub window_hung: DurationRule,
    pub app_crash: DurationRule,
    /// Reset margin below the threshold (percentage points or degrees).
    pub hysteresis: f32,
    /// How long the value must stay below `threshold - hysteresis` to reset a rule.
    pub hysteresis_hold_s: u64,
    pub cooldown_s: u64,
    pub max_voice_per_hour: u32,
    pub quiet_hours: QuietHours,
}

impl Default for ThresholdRule {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 90.0,
            duration_s: 60,
        }
    }
}

impl Default for DurationRule {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_s: 0,
        }
    }
}

impl Default for DiskRule {
    fn default() -> Self {
        Self {
            enabled: true,
            min_free_gb: 10.0,
            min_free_percent: 5.0,
            check_every_s: 600,
        }
    }
}

impl Default for QuietHours {
    fn default() -> Self {
        Self {
            enabled: false,
            start_minute: 22 * 60,
            end_minute: 7 * 60,
        }
    }
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            cpu_load: ThresholdRule {
                enabled: true,
                threshold: 90.0,
                duration_s: 60,
            },
            gpu_temp: ThresholdRule {
                enabled: true,
                threshold: 83.0,
                duration_s: 30,
            },
            cpu_temp: ThresholdRule {
                enabled: true,
                threshold: 90.0,
                duration_s: 30,
            },
            ram_load: ThresholdRule {
                enabled: true,
                threshold: 92.0,
                duration_s: 30,
            },
            disk_space: DiskRule::default(),
            internet_down: DurationRule {
                enabled: true,
                duration_s: 20,
            },
            window_hung: DurationRule {
                enabled: true,
                duration_s: 15,
            },
            app_crash: DurationRule {
                enabled: true,
                duration_s: 0,
            },
            hysteresis: 5.0,
            hysteresis_hold_s: 30,
            cooldown_s: 15 * 60,
            max_voice_per_hour: 3,
            quiet_hours: QuietHours::default(),
        }
    }
}

impl QuietHours {
    /// True when `local_minute` (minutes since local midnight) falls inside the quiet range.
    pub fn contains(&self, local_minute: u16) -> bool {
        if !self.enabled || self.start_minute == self.end_minute {
            return false;
        }
        if self.start_minute < self.end_minute {
            local_minute >= self.start_minute && local_minute < self.end_minute
        } else {
            local_minute >= self.start_minute || local_minute < self.end_minute
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_is_slower_than_window() {
        let tray = Rates::for_mode(Mode::Tray);
        let window = Rates::for_mode(Mode::Window);
        assert_eq!(tray.cpu_ram, Duration::from_secs(5));
        assert_eq!(window.cpu_ram, Duration::from_secs(1));
        assert_eq!(tray.gpu, Duration::from_secs(10));
        assert_eq!(window.gpu, Duration::from_secs(1));
        assert_eq!(tray.rtt, Duration::from_secs(60));
        assert_eq!(window.rtt, Duration::from_secs(10));
        assert_eq!(tray.net_io, Duration::from_secs(10));
        assert_eq!(window.net_io, Duration::from_secs(1));
        assert_eq!(tray.hung_windows, Duration::from_secs(10));
        assert!(window.volumes < tray.volumes);
    }

    #[test]
    fn quiet_hours_wrap_over_midnight() {
        let quiet = QuietHours {
            enabled: true,
            start_minute: 22 * 60,
            end_minute: 7 * 60,
        };
        assert!(quiet.contains(23 * 60));
        assert!(quiet.contains(3 * 60));
        assert!(!quiet.contains(12 * 60));
        assert!(!QuietHours::default().contains(23 * 60));
    }
}
