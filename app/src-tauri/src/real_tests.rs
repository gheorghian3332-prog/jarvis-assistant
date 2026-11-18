//! Real-machine checks of the whole command path: a typed phrase goes through the FAST PATH, the
//! real executor and the real system intelligence / screen vision, and the answer is read back.
//!
//! The screen check uses a window of its own (a tiny Win32 window with known text, closed again at
//! once): never Notepad, never input synthesis, never the focus. The window is created off screen,
//! without activation, and the vision backend is pointed at it directly, so whatever the user is
//! doing (a game, typing) is not touched.

use crate::{
    alerts::{AlertPayload, AlertPipeline, AlertShell},
    dispatch::{
        ActionReport, AssistantMessage, CommandCore, ConfirmRequest, Engine, Shell, Source,
    },
    insight::{self, RealVision, VisionSettings},
};
use async_trait::async_trait;
use jarvis_brain::{config as brain_config, BrainConfig, Ollama};
use jarvis_control::{ControlConfig, MemoryAudit};
use jarvis_core::{EventBus, Service};
use jarvis_sysintel::{gpu::GpuSource, SysIntel, SysIntelConfig};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct RecordingShell {
    state: Mutex<String>,
    messages: Mutex<Vec<String>>,
    spoken: Mutex<Vec<String>>,
}

impl Shell for RecordingShell {
    fn set_state(&self, state: &str) {
        *lock(&self.state) = state.to_owned();
    }
    fn state(&self) -> String {
        lock(&self.state).clone()
    }
    fn speak(&self, text: &str) {
        lock(&self.spoken).push(text.to_owned());
    }
    fn message(&self, message: &AssistantMessage) {
        lock(&self.messages).push(message.text.clone());
    }
    fn confirm_request(&self, _request: &ConfirmRequest) {}
    fn action_result(&self, _report: &ActionReport) {}
    fn silence(&self) {}
}

struct Rig {
    core: Arc<CommandCore>,
    shell: Arc<RecordingShell>,
    sys: Arc<SysIntel>,
    runner: tokio::task::JoinHandle<()>,
    indicator: Arc<Mutex<Vec<bool>>>,
}

fn rig(settings: VisionSettings, target: Option<isize>) -> Rig {
    let bus = EventBus::new();
    let sys =
        Arc::new(SysIntel::new(bus, SysIntelConfig::default()).with_gpu_source(GpuSource::D3dkmt));
    let runner = {
        let sys = Arc::clone(&sys);
        tokio::spawn(async move {
            let _ = sys.start().await;
        })
    };
    let indicator = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&indicator);
    let observer = Arc::new(insight::IndicatorObserver::new(move |active| {
        lock(&sink).push(active);
    }));
    let mut backend = RealVision::new(
        Arc::new(RwLock::new(settings)),
        brain_config::shared(BrainConfig::default()),
        Arc::new(Ollama::local().expect("local ollama client")),
        observer,
    );
    if let Some(hwnd) = target {
        backend = backend.with_target_window(hwnd);
    }
    let backend = Arc::new(backend);
    let audit = Arc::new(MemoryAudit::default());
    let hub = Arc::new(crate::control::ControlHub::new(
        ControlConfig::default(),
        audit.clone(),
        insight::actions(sys.clone(), backend),
    ));
    let shell = Arc::new(RecordingShell::default());
    let engine: Arc<dyn Engine> = hub;
    let core = CommandCore::new(engine, shell.clone(), audit);
    Rig {
        core,
        shell,
        sys,
        runner,
        indicator,
    }
}

async fn stop(rig: &Rig) {
    let _ = rig.sys.stop().await;
    rig.runner.abort();
}

/// The first number in `text`.
fn first_number(text: &str) -> Option<u32> {
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[tokio::test(flavor = "current_thread")]
async fn real_chat_gpu_temperature_gives_the_real_number() {
    let rig = rig(VisionSettings::default(), None);
    // The tray reads the GPU through D3DKMT; wait for its first reading.
    for _ in 0..100 {
        if rig.sys.snapshot().gpu.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let Some(gpu) = rig.sys.snapshot().gpu else {
        eprintln!("no GPU reading on this machine: the answer must say it is unavailable");
        rig.core
            .submit("какая температура видеокарты", None, Source::Text)
            .await;
        let said = lock(&rig.shell.messages).clone();
        assert!(said.iter().any(|m| m.contains("не удалось")), "{said:?}");
        stop(&rig).await;
        return;
    };
    rig.core
        .submit("какая температура видеокарты", None, Source::Text)
        .await;
    let said = lock(&rig.shell.messages).clone();
    eprintln!("chat_send \"какая температура видеокарты\" -> {said:?}");
    assert_eq!(said.len(), 1, "{said:?}");
    let reported = first_number_after_dash(&said[0]).expect("a number in the answer");
    let real = gpu.temperature_c.expect("temperature");
    assert!(
        reported.abs_diff(real) <= 3,
        "answer {reported} vs reading {real}"
    );
    assert!(lock(&rig.shell.spoken).len() == 1);
    // Memory and CPU answers carry real numbers too.
    for (phrase, needle) in [
        ("сколько свободно памяти", "Свободно"),
        ("загрузка процессора", "Процессор загружен"),
    ] {
        rig.core.submit(phrase, None, Source::Text).await;
        let said = lock(&rig.shell.messages).clone();
        let last = said.last().cloned().unwrap_or_default();
        eprintln!("chat_send \"{phrase}\" -> {last}");
        assert!(
            last.contains(needle) && first_number(&last).is_some(),
            "{last}"
        );
    }
    stop(&rig).await;
}

/// The temperature is the number after the dash ("... 1660 — 74 градуса").
fn first_number_after_dash(text: &str) -> Option<u32> {
    first_number(text.rsplit('—').next()?)
}

// ---------- a window of our own ----------

mod window {
    use std::sync::mpsc;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateFontW, DeleteObject, EndPaint, FillRect, GetStockObject, SelectObject,
        SetBkMode, SetTextColor, TextOutW, HBRUSH, HDC, PAINTSTRUCT, TRANSPARENT, WHITE_BRUSH,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, PostMessageW, PostQuitMessage, RegisterClassExW, ShowWindow, TranslateMessage,
        CS_HREDRAW, CS_VREDRAW, MSG, SW_SHOWNOACTIVATE, WM_CLOSE, WM_DESTROY, WM_PAINT,
        WM_PRINTCLIENT, WNDCLASSEXW, WS_CAPTION, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_SYSMENU,
        WS_VISIBLE,
    };

    pub const LINE_ONE: &str = "JARVIS TEST WINDOW 4821";
    pub const LINE_TWO: &str = "Hello from the vision check";

    /// Paints the white background and the two known lines.
    unsafe fn draw(hwnd: HWND, hdc: HDC) {
        let mut area = windows::Win32::Foundation::RECT::default();
        let _ = GetClientRect(hwnd, &mut area);
        FillRect(hdc, &area, HBRUSH(GetStockObject(WHITE_BRUSH).0));
        let font = CreateFontW(
            -44,
            0,
            0,
            0,
            700,
            0,
            0,
            0,
            windows::Win32::Graphics::Gdi::DEFAULT_CHARSET.0 as u32,
            windows::Win32::Graphics::Gdi::OUT_DEFAULT_PRECIS.0 as u32,
            windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS.0 as u32,
            windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY.0 as u32,
            0,
            w!("Arial"),
        );
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(0));
        let one: Vec<u16> = LINE_ONE.encode_utf16().collect();
        let two: Vec<u16> = LINE_TWO.encode_utf16().collect();
        let _ = TextOutW(hdc, 20, 20, &one);
        let _ = TextOutW(hdc, 20, 90, &two);
        SelectObject(hdc, old);
        let _ = DeleteObject(font);
    }

    unsafe extern "system" fn procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint);
                draw(hwnd, hdc);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            // PrintWindow asks the window to render itself into the given device context.
            WM_PRINTCLIENT => {
                draw(hwnd, HDC(wparam.0 as *mut _));
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }

    /// A small window with known text on its own thread, placed off screen and never activated.
    pub struct TestWindow {
        hwnd: isize,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl TestWindow {
        pub fn open() -> Option<Self> {
            let (sender, receiver) = mpsc::channel::<isize>();
            let thread = std::thread::spawn(move || {
                // SAFETY: plain Win32 window creation and a message loop on this thread.
                unsafe {
                    let instance: windows::Win32::Foundation::HINSTANCE =
                        GetModuleHandleW(PCWSTR::null())
                            .map(Into::into)
                            .unwrap_or_default();
                    let class = w!("JarvisVisionTestWindow");
                    let class_info = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        style: CS_HREDRAW | CS_VREDRAW,
                        lpfnWndProc: Some(procedure),
                        hInstance: instance,
                        lpszClassName: class,
                        ..Default::default()
                    };
                    RegisterClassExW(&class_info);
                    let hwnd = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        class,
                        w!("Jarvis vision test"),
                        WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                        -4000,
                        -4000,
                        1000,
                        230,
                        None,
                        None,
                        instance,
                        None,
                    );
                    let Ok(hwnd) = hwnd else {
                        let _ = sender.send(0);
                        return;
                    };
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    let _ = sender.send(hwnd.0 as isize);
                    let mut message = MSG::default();
                    while GetMessageW(&mut message, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            });
            let hwnd = receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .ok()?;
            (hwnd != 0).then_some(Self {
                hwnd,
                thread: Some(thread),
            })
        }

        pub fn hwnd(&self) -> isize {
            self.hwnd
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: posting a close message to a window of this process.
            unsafe {
                let _ = PostMessageW(HWND(self.hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn real_chat_reads_the_text_of_our_own_window() {
    let Some(test_window) = window::TestWindow::open() else {
        panic!("the test window could not be created");
    };
    let rig = rig(VisionSettings::default(), Some(test_window.hwnd()));
    // Let the window paint.
    tokio::time::sleep(Duration::from_millis(500)).await;
    rig.core
        .submit("что написано в этом окне", None, Source::Text)
        .await;
    drop(test_window);
    let said = lock(&rig.shell.messages).clone();
    eprintln!("chat_send \"что написано в этом окне\" -> {said:?}");
    eprintln!("capture indicator: {:?}", lock(&rig.indicator));
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("JARVIS") && said[0].contains("4821"),
        "OCR text expected in the answer: {}",
        said[0]
    );
    // The indicator went up and came down again.
    assert_eq!(*lock(&rig.indicator), [true, false]);
    // The chat shows the whole text; the voice a short part of it.
    assert!(lock(&rig.shell.spoken).len() == 1);
    stop(&rig).await;
}

#[tokio::test(flavor = "current_thread")]
async fn vision_switched_off_in_settings_refuses_and_never_touches_the_screen() {
    let rig = rig(
        VisionSettings {
            enabled: false,
            ..VisionSettings::default()
        },
        None,
    );
    rig.core
        .submit("что написано в этом окне", None, Source::Text)
        .await;
    let said = lock(&rig.shell.messages).clone();
    assert_eq!(said.len(), 1);
    assert!(said[0].contains("выключено"), "{said:?}");
    assert!(lock(&rig.indicator).is_empty(), "no capture, no indicator");
    stop(&rig).await;
}

// ---------- the alert path, end to end ----------

#[derive(Default)]
struct AlertRecorder {
    raised: Mutex<Vec<AlertPayload>>,
}

#[async_trait]
impl AlertShell for AlertRecorder {
    fn window_open(&self) -> bool {
        true
    }
    fn raise(&self, payload: &AlertPayload) {
        lock(&self.raised).push(payload.clone());
    }
    fn notify(&self, _title: &str, _body: &str) {}
    fn voice_state(&self) -> String {
        "SPEAKING".into()
    }
    fn show_state(&self, _state: &str) {}
    fn speak(&self, _text: &str) {}
    async fn english(&self) -> bool {
        false
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_synthetic_alert_travels_from_the_service_to_the_pipeline() {
    use jarvis_sysintel::alerts::{Alert, AlertKind, Severity};
    let bus = EventBus::new();
    let sys = SysIntel::new(bus.clone(), SysIntelConfig::default());
    let shell = Arc::new(AlertRecorder::default());
    let task = tokio::spawn(AlertPipeline::new(shell.clone()).run(bus.clone()));
    tokio::task::yield_now().await;
    sys.inject_event(jarvis_sysintel::SysEvent::Alert(Alert {
        id: "debug-1".into(),
        kind: AlertKind::GpuTemp,
        severity: Severity::Critical,
        message_ru: "Видеокарта перегрета".into(),
        message_en: "The GPU is overheating".into(),
        speak: true,
    }));
    for _ in 0..50 {
        if !lock(&shell.raised).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let raised = lock(&shell.raised).clone();
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0].kind, "gpu_temp");
    assert_eq!(raised[0].severity, "critical");
    assert_eq!(sys.recent_alerts(5).len(), 1);
    task.abort();
}
