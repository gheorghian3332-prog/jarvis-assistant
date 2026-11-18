//! COM apartment guard and shell launch helper shared by the controllers.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::error::{ControlError, ControlResult};

/// Initializes COM on the current thread and balances it on drop.
pub struct ComGuard {
    uninitialize: bool,
}

impl ComGuard {
    pub fn init() -> Self {
        // SAFETY: plain COM initialization for the calling thread.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // S_OK / S_FALSE both require a matching CoUninitialize; a failure (e.g. a different
        // apartment model already set) does not.
        Self {
            uninitialize: result.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the successful CoInitializeEx in `init` on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// `ShellExecuteW(open, target, args)`. Handles files, folders, URIs (`steam://`, `ms-settings:`)
/// and `shell:AppsFolder\...` monikers.
pub fn shell_open(target: &str, arguments: Option<&str>) -> ControlResult<()> {
    let target_w = HSTRING::from(target);
    let args_w = arguments.map(HSTRING::from);
    let args_ptr = args_w
        .as_ref()
        .map_or(PCWSTR::null(), |text| PCWSTR(text.as_ptr()));
    // SAFETY: all string pointers stay alive for the duration of the call.
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            windows::core::w!("open"),
            PCWSTR(target_w.as_ptr()),
            args_ptr,
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Per the ShellExecute contract, values above 32 mean success.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(ControlError::system(format!(
            "ShellExecuteW failed for {target} (code {})",
            result.0 as isize
        )))
    }
}
