//! The `sysintel.*` settings (alert rules, quiet hours, RTT host): validation and loading.

use crate::{error::CommandError, settings};
use jarvis_core::Database;
use jarvis_sysintel::config::{DiskRule, DurationRule, ThresholdRule};
use jarvis_sysintel::{AlertConfig, SysIntelConfig};
use serde_json::Value;

fn invalid(key: &str, message: impl std::fmt::Display) -> CommandError {
    CommandError::Validation(format!("{key}: {message}"))
}

fn within(key: &str, what: &str, value: f32, min: f32, max: f32) -> Result<(), CommandError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(invalid(key, format!("{what} must be from {min} to {max}")))
    }
}

fn seconds(key: &str, what: &str, value: u64, max: u64) -> Result<(), CommandError> {
    if value <= max {
        Ok(())
    } else {
        Err(invalid(key, format!("{what} must be at most {max}")))
    }
}

fn threshold(
    key: &str,
    name: &str,
    rule: &ThresholdRule,
    min: f32,
    max: f32,
) -> Result<(), CommandError> {
    within(key, &format!("{name}.threshold"), rule.threshold, min, max)?;
    seconds(key, &format!("{name}.duration_s"), rule.duration_s, 3600)
}

fn duration(key: &str, name: &str, rule: &DurationRule) -> Result<(), CommandError> {
    seconds(key, &format!("{name}.duration_s"), rule.duration_s, 3600)
}

fn disk(key: &str, rule: &DiskRule) -> Result<(), CommandError> {
    within(key, "disk_space.min_free_gb", rule.min_free_gb, 0.0, 2000.0)?;
    within(
        key,
        "disk_space.min_free_percent",
        rule.min_free_percent,
        0.0,
        50.0,
    )?;
    if (10..=86_400).contains(&rule.check_every_s) {
        Ok(())
    } else {
        Err(invalid(
            key,
            "disk_space.check_every_s must be from 10 to 86400",
        ))
    }
}

/// Parses and range-checks the alert rules. Missing fields take their defaults.
pub fn parse_alerts(key: &str, value: &Value) -> Result<AlertConfig, CommandError> {
    if !value.is_object() {
        return Err(invalid(key, "expected an object with the alert rules"));
    }
    let config: AlertConfig = serde_json::from_value(value.clone())
        .map_err(|error| invalid(key, format!("not an alert configuration: {error}")))?;
    threshold(key, "cpu_load", &config.cpu_load, 50.0, 100.0)?;
    threshold(key, "gpu_temp", &config.gpu_temp, 40.0, 120.0)?;
    threshold(key, "cpu_temp", &config.cpu_temp, 40.0, 120.0)?;
    threshold(key, "ram_load", &config.ram_load, 50.0, 100.0)?;
    disk(key, &config.disk_space)?;
    duration(key, "internet_down", &config.internet_down)?;
    duration(key, "window_hung", &config.window_hung)?;
    duration(key, "app_crash", &config.app_crash)?;
    within(key, "hysteresis", config.hysteresis, 0.0, 30.0)?;
    seconds(key, "hysteresis_hold_s", config.hysteresis_hold_s, 3600)?;
    seconds(key, "cooldown_s", config.cooldown_s, 86_400)?;
    if config.max_voice_per_hour > 60 {
        return Err(invalid(key, "max_voice_per_hour must be at most 60"));
    }
    let quiet = config.quiet_hours;
    if quiet.start_minute >= 1440 || quiet.end_minute >= 1440 {
        return Err(invalid(key, "quiet_hours minutes must be below 1440"));
    }
    Ok(config)
}

/// `host:port` for the latency probe: a DNS name or IPv4 address and a port.
pub fn parse_rtt_host(key: &str, value: &Value) -> Result<String, CommandError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid(key, "expected text such as 1.1.1.1:443"))?;
    let bad = || invalid(key, "expected host:port, for example 1.1.1.1:443");
    let (host, port) = text.rsplit_once(':').ok_or_else(bad)?;
    let port_ok = port.parse::<u16>().is_ok_and(|p| p > 0);
    let host_ok = !host.is_empty()
        && host.len() <= 100
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    if port_ok && host_ok {
        Ok(text.to_owned())
    } else {
        Err(bad())
    }
}

/// Validates any `sysintel.*` value.
pub fn validate(key: &str, value: &Value) -> Result<(), CommandError> {
    match key {
        settings::SYSINTEL_ALERTS => parse_alerts(key, value).map(|_| ()),
        settings::SYSINTEL_RTT_HOST => parse_rtt_host(key, value).map(|_| ()),
        _ => Err(invalid(key, "not a known setting")),
    }
}

/// Reads the stored settings; invalid values fall back to the defaults.
pub async fn load_config(db: &Database) -> SysIntelConfig {
    let read = |key: &'static str| {
        let db = db.clone();
        async move { db.settings_get(key.to_owned()).await.ok().flatten() }
    };
    let mut config = SysIntelConfig::default();
    if let Some(alerts) = read(settings::SYSINTEL_ALERTS)
        .await
        .and_then(|v| parse_alerts(settings::SYSINTEL_ALERTS, &v).ok())
    {
        config.alerts = alerts;
    }
    if let Some(host) = read(settings::SYSINTEL_RTT_HOST)
        .await
        .and_then(|v| parse_rtt_host(settings::SYSINTEL_RTT_HOST, &v).ok())
    {
        config.rtt_host = host;
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_defaults_are_a_valid_configuration() {
        let value = serde_json::to_value(AlertConfig::default()).expect("serialize");
        let parsed = parse_alerts("k", &value).expect("valid");
        assert_eq!(parsed, AlertConfig::default());
        // a partial object fills in the rest
        let partial = parse_alerts("k", &json!({"gpu_temp": {"threshold": 80.0}})).expect("valid");
        assert_eq!(partial.gpu_temp.threshold, 80.0);
        assert_eq!(partial.cpu_load, AlertConfig::default().cpu_load);
    }

    #[test]
    fn out_of_range_rules_are_rejected() {
        for bad in [
            json!({"cpu_load": {"threshold": 20.0}}),
            json!({"gpu_temp": {"threshold": 200.0}}),
            json!({"ram_load": {"duration_s": 99999}}),
            json!({"disk_space": {"check_every_s": 1}}),
            json!({"disk_space": {"min_free_percent": 80.0}}),
            json!({"cooldown_s": 999999}),
            json!({"max_voice_per_hour": 500}),
            json!({"quiet_hours": {"start_minute": 1500}}),
            json!({"cpu_load": {"enabled": "yes"}}),
            json!({"hysteresis": -1.0}),
            json!("text"),
            json!(null),
        ] {
            assert_eq!(
                parse_alerts("k", &bad).expect_err("invalid").code(),
                "VALIDATION",
                "{bad}"
            );
        }
    }

    #[test]
    fn quiet_hours_and_switches_are_accepted() {
        let value = json!({
            "quiet_hours": {"enabled": true, "start_minute": 1380, "end_minute": 420},
            "gpu_temp": {"enabled": false}
        });
        let parsed = parse_alerts("k", &value).expect("valid");
        assert!(parsed.quiet_hours.enabled && !parsed.gpu_temp.enabled);
        assert!(parsed.quiet_hours.contains(23 * 60 + 30));
    }

    #[test]
    fn the_rtt_host_is_a_host_and_a_port() {
        for good in ["1.1.1.1:443", "example.com:80", "my-host.local:8080"] {
            assert!(parse_rtt_host("k", &json!(good)).is_ok(), "{good}");
        }
        for bad in [
            "1.1.1.1",
            "1.1.1.1:0",
            "1.1.1.1:70000",
            ":443",
            "a b:443",
            "http://x:443",
            "-x:443",
        ] {
            assert!(parse_rtt_host("k", &json!(bad)).is_err(), "{bad}");
        }
        assert!(parse_rtt_host("k", &json!(5)).is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(validate("sysintel.nothing", &json!(1)).is_err());
    }
}
