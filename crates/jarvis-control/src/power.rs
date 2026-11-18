//! Power: lock, sleep/hibernate, delayed shutdown/restart (`InitiateShutdownW`) and abort.

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_SHUTDOWN_IN_PROGRESS, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::{
    AbortSystemShutdownW, InitiateShutdownW, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FLAGS, SHUTDOWN_POWEROFF,
    SHUTDOWN_RESTART,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::error::{ControlError, ControlResult};

/// Grace period before a scheduled shutdown/restart actually happens.
pub const SHUTDOWN_DELAY_SECONDS: u32 = 30;

pub fn lock() -> ControlResult<()> {
    // SAFETY: no arguments.
    unsafe { LockWorkStation()? };
    Ok(())
}

/// Suspends (`hibernate == false`) or hibernates the machine.
pub fn suspend(hibernate: bool) -> ControlResult<()> {
    // SAFETY: plain call; not forced, wake events stay enabled.
    let ok = unsafe { SetSuspendState(hibernate, false, false) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(ControlError::system(if hibernate {
            "hibernation failed or is disabled"
        } else {
            "sleep failed"
        }))
    }
}

fn enable_shutdown_privilege() -> ControlResult<()> {
    // SAFETY: token handle is closed on every path; structures are local and fully initialized.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?;
        let mut luid = LUID::default();
        let lookup = LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid);
        if let Err(error) = lookup {
            let _ = CloseHandle(token);
            return Err(error.into());
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let adjusted = AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None);
        let _ = CloseHandle(token);
        adjusted?;
    }
    Ok(())
}

/// Schedules a shutdown or restart after [`SHUTDOWN_DELAY_SECONDS`]; running applications are asked
/// to close normally (no forced kill). Cancel with [`abort_shutdown`].
pub fn schedule_shutdown(restart: bool) -> ControlResult<()> {
    enable_shutdown_privilege()?;
    let message = HSTRING::from(if restart {
        "J.A.R.V.I.S.: scheduled restart"
    } else {
        "J.A.R.V.I.S.: scheduled shutdown"
    });
    let flags = SHUTDOWN_FLAGS(if restart {
        SHUTDOWN_RESTART.0
    } else {
        SHUTDOWN_POWEROFF.0
    });
    let reason = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
    // SAFETY: the message string outlives the call; local machine (null name).
    let code = unsafe { InitiateShutdownW(None, &message, SHUTDOWN_DELAY_SECONDS, flags, reason) };
    if code == 0 {
        Ok(())
    } else {
        Err(ControlError::system(format!(
            "InitiateShutdownW failed with code {code}"
        )))
    }
}

/// Cancels a scheduled shutdown. Returns `false` when nothing was scheduled.
pub fn abort_shutdown() -> ControlResult<bool> {
    enable_shutdown_privilege()?;
    // SAFETY: local machine (null name).
    match unsafe { AbortSystemShutdownW(None) } {
        Ok(()) => Ok(true),
        Err(error) if error.code() == ERROR_NO_SHUTDOWN_IN_PROGRESS.to_hresult() => Ok(false),
        Err(error) => Err(error.into()),
    }
}
