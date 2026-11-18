//! Unexpected application exits: a push subscription (`EvtSubscribe`) to the `Application` event log
//! for Application Error (event 1000) and Application Hang (1002). Reading that log needs no admin
//! rights, and a subscription costs nothing while no event arrives.

use crate::metrics::AppCrashed;
use std::ffi::c_void;
use tokio::sync::mpsc::UnboundedSender;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::System::EventLog::{
    EvtClose, EvtRender, EvtRenderEventXml, EvtSubscribe, EvtSubscribeActionDeliver,
    EvtSubscribeToFutureEvents, EVT_HANDLE, EVT_SUBSCRIBE_NOTIFY_ACTION,
};

const QUERY: PCWSTR = w!(
    "*[System[Provider[@Name='Application Error' or @Name='Application Hang'] and (EventID=1000 or EventID=1002)]]"
);

/// Parses the XML of an Application Error / Application Hang event.
pub fn parse_crash_event(xml: &str) -> Option<AppCrashed> {
    let id: u32 = element_text(xml, "EventID")?.trim().parse().ok()?;
    let provider = attribute_of(xml, "Provider", "Name")?;
    let expected = match id {
        1000 => "Application Error",
        1002 => "Application Hang",
        _ => return None,
    };
    if provider != expected {
        return None;
    }
    let time = attribute_of(xml, "TimeCreated", "SystemTime")?;
    let time_ms = parse_iso_time_ms(&time)?;
    // The first <Data> of both events is the faulting program (`chrome.exe`).
    let data_start = xml.find("<EventData")?;
    let name = element_text(&xml[data_start..], "Data")?;
    let name = unescape(name.trim());
    if name.is_empty() {
        return None;
    }
    Some(AppCrashed { name, time_ms })
}

/// Text between `<tag ...>` and `</tag>` of the first such element.
fn element_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let mut from = 0;
    loop {
        let start = from + xml[from..].find(&open)?;
        let after = start + open.len();
        // Must be the whole tag name, not a longer one (`<Data` vs `<DataFoo`).
        match xml[after..].chars().next()? {
            '>' | ' ' | '/' | '\t' | '\r' | '\n' => {}
            _ => {
                from = after;
                continue;
            }
        }
        let tag_end = after + xml[after..].find('>')?;
        if xml[..tag_end].ends_with('/') {
            return Some("");
        }
        let body = tag_end + 1;
        let close = body + xml[body..].find(&format!("</{tag}>"))?;
        return Some(&xml[body..close]);
    }
}

/// Value of `attr` on the first `<tag ...>` element.
fn attribute_of(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let tag_end = start + xml[start..].find('>')?;
    let head = &xml[start..tag_end];
    for quote in ['\'', '"'] {
        let key = format!("{attr}={quote}");
        if let Some(pos) = head.find(&key) {
            let value_start = pos + key.len();
            let value_end = value_start + head[value_start..].find(quote)?;
            return Some(unescape(&head[value_start..value_end]));
        }
    }
    None
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// `2026-11-18T10:15:30.1234567Z` to Unix milliseconds.
pub fn parse_iso_time_ms(text: &str) -> Option<u64> {
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let (hms, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let second: i64 = t.next()?.parse().ok()?;
    let millis: i64 = format!("{:0<3}", fraction)[..3].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 24 + hour) * 60 + minute) * 60 + second;
    u64::try_from(ms * 1000 + millis).ok()
}

struct CallbackContext {
    sender: UnboundedSender<AppCrashed>,
}

/// Live subscription; dropping it unsubscribes.
pub struct CrashWatcher {
    handle: EVT_HANDLE,
    context: *mut CallbackContext,
}

// The handle is an opaque kernel id and the context is only used behind the callback.
unsafe impl Send for CrashWatcher {}
unsafe impl Sync for CrashWatcher {}

impl CrashWatcher {
    /// Starts watching for events raised from now on.
    pub fn start(sender: UnboundedSender<AppCrashed>) -> windows::core::Result<Self> {
        let context = Box::into_raw(Box::new(CallbackContext { sender }));
        // SAFETY: `context` stays valid until `drop`, which closes the subscription first.
        let result = unsafe {
            EvtSubscribe(
                EVT_HANDLE::default(),
                None,
                w!("Application"),
                QUERY,
                EVT_HANDLE::default(),
                Some(context as *const c_void),
                Some(on_event),
                EvtSubscribeToFutureEvents.0,
            )
        };
        match result {
            Ok(handle) => Ok(Self { handle, context }),
            Err(error) => {
                // SAFETY: the subscription failed, so nothing else owns the context.
                drop(unsafe { Box::from_raw(context) });
                Err(error)
            }
        }
    }
}

impl Drop for CrashWatcher {
    fn drop(&mut self) {
        // SAFETY: EvtClose waits for running callbacks, after which the context is unused.
        unsafe {
            let _ = EvtClose(self.handle);
            drop(Box::from_raw(self.context));
        }
    }
}

unsafe extern "system" fn on_event(
    action: EVT_SUBSCRIBE_NOTIFY_ACTION,
    context: *const c_void,
    event: EVT_HANDLE,
) -> u32 {
    if action != EvtSubscribeActionDeliver || context.is_null() {
        return 0;
    }
    // SAFETY: `context` is the pointer given to EvtSubscribe and outlives the subscription.
    let context = unsafe { &*(context as *const CallbackContext) };
    if let Some(xml) = unsafe { render_xml(event) } {
        if let Some(crash) = parse_crash_event(&xml) {
            let _ = context.sender.send(crash);
        }
    }
    0
}

/// # Safety
/// `event` must be a valid event handle.
unsafe fn render_xml(event: EVT_HANDLE) -> Option<String> {
    let mut used = 0u32;
    let mut count = 0u32;
    let first = EvtRender(
        EVT_HANDLE::default(),
        event,
        EvtRenderEventXml.0,
        0,
        None,
        &mut used,
        &mut count,
    );
    match first {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        _ => return None,
    }
    let mut buffer = vec![0u16; (used as usize).div_ceil(2) + 1];
    let bytes = (buffer.len() * 2) as u32;
    EvtRender(
        EVT_HANDLE::default(),
        event,
        EvtRenderEventXml.0,
        bytes,
        Some(buffer.as_mut_ptr().cast()),
        &mut used,
        &mut count,
    )
    .ok()?;
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ERROR_1000: &str = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Application Error'/><EventID Qualifiers='0'>1000</EventID><Version>0</Version><Level>2</Level><TimeCreated SystemTime='2026-11-18T10:15:30.1234567Z'/><EventRecordID>1</EventRecordID><Channel>Application</Channel><Computer>PC</Computer></System><EventData><Data>chrome.exe</Data><Data>120.0.6099.71</Data><Data>6550a1b2</Data><Data>KERNELBASE.dll</Data></EventData></Event>"#;

    const HANG_1002: &str = r#"<Event xmlns="http://schemas.microsoft.com/win/2004/08/events/event"><System><Provider Name="Application Hang"/><EventID Qualifiers="0">1002</EventID><TimeCreated SystemTime="2026-11-18T23:59:59.5Z"/></System><EventData><Data>My &amp; App.exe</Data><Data>1.0</Data></EventData></Event>"#;

    const OTHER: &str = r#"<Event><System><Provider Name='Application Error'/><EventID>1001</EventID><TimeCreated SystemTime='2026-11-18T10:15:30.0Z'/></System><EventData><Data>x.exe</Data></EventData></Event>"#;

    #[test]
    fn parses_application_error() {
        let crash = parse_crash_event(ERROR_1000).expect("crash");
        assert_eq!(crash.name, "chrome.exe");
        // 2026-11-18T10:15:30.123Z
        assert_eq!(crash.time_ms, 1_794_996_930_123);
    }

    #[test]
    fn parses_application_hang_with_entities() {
        let crash = parse_crash_event(HANG_1002).expect("crash");
        assert_eq!(crash.name, "My & App.exe");
        assert_eq!(crash.time_ms % 1000, 500);
    }

    #[test]
    fn ignores_other_events_and_garbage() {
        assert!(parse_crash_event(OTHER).is_none());
        assert!(parse_crash_event("").is_none());
        assert!(parse_crash_event("<Event><System></System></Event>").is_none());
    }

    #[test]
    fn iso_time_matches_known_dates() {
        assert_eq!(parse_iso_time_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_time_ms("2000-03-01T00:00:01.5Z"),
            Some(951_868_801_500)
        );
        assert_eq!(parse_iso_time_ms("nonsense"), None);
    }
}
