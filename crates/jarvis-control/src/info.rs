//! Local time and date.

use windows::Win32::System::SystemInformation::GetLocalTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Sunday.
    pub weekday: usize,
}

pub fn now() -> LocalTime {
    // SAFETY: GetLocalTime has no failure mode and returns a plain struct.
    let time = unsafe { GetLocalTime() };
    LocalTime {
        year: u32::from(time.wYear),
        month: u32::from(time.wMonth),
        day: u32::from(time.wDay),
        hour: u32::from(time.wHour),
        minute: u32::from(time.wMinute),
        second: u32::from(time.wSecond),
        weekday: usize::from(time.wDayOfWeek),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_time_is_sane() {
        let time = now();
        assert!(time.year >= 2024);
        assert!((1..=12).contains(&time.month));
        assert!((1..=31).contains(&time.day));
        assert!(time.hour < 24 && time.minute < 60 && time.second < 61);
        assert!(time.weekday < 7);
    }
}
