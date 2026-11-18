//! Master volume (`IAudioEndpointVolume`) and media keys (`WM_APPCOMMAND` to the shell window).

use windows::core::w;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_APPCOMMAND};

use crate::com::ComGuard;
use crate::error::{ControlError, ControlResult};

const APPCOMMAND_MEDIA_NEXTTRACK: isize = 11;
const APPCOMMAND_MEDIA_PREVIOUSTRACK: isize = 12;
const APPCOMMAND_MEDIA_PLAY_PAUSE: isize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Prev,
}

impl MediaKey {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKey::PlayPause => "play_pause",
            MediaKey::Next => "next",
            MediaKey::Prev => "prev",
        }
    }
    fn command(self) -> isize {
        match self {
            MediaKey::PlayPause => APPCOMMAND_MEDIA_PLAY_PAUSE,
            MediaKey::Next => APPCOMMAND_MEDIA_NEXTTRACK,
            MediaKey::Prev => APPCOMMAND_MEDIA_PREVIOUSTRACK,
        }
    }
}

fn endpoint() -> ControlResult<IAudioEndpointVolume> {
    // SAFETY: COM calls on an initialized apartment (the caller holds a `ComGuard`).
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .map_err(|error| {
                ControlError::unavailable(format!("no audio output device: {error}"))
            })?;
        Ok(device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?)
    }
}

/// Master volume as the raw scalar in `0.0..=1.0`.
pub fn get_scalar() -> ControlResult<f32> {
    let _com = ComGuard::init();
    let volume = endpoint()?;
    // SAFETY: valid interface pointer.
    Ok(unsafe { volume.GetMasterVolumeLevelScalar()? })
}

/// Sets the raw scalar (clamped to `0.0..=1.0`).
pub fn set_scalar(value: f32) -> ControlResult<()> {
    let _com = ComGuard::init();
    let volume = endpoint()?;
    // SAFETY: valid interface pointer; null event context.
    unsafe { volume.SetMasterVolumeLevelScalar(value.clamp(0.0, 1.0), std::ptr::null())? };
    Ok(())
}

/// Master volume in whole percent.
pub fn get_volume() -> ControlResult<u32> {
    Ok((get_scalar()? * 100.0).round() as u32)
}

pub fn set_volume(percent: u32) -> ControlResult<u32> {
    let percent = percent.min(100);
    set_scalar(percent as f32 / 100.0)?;
    Ok(percent)
}

/// Relative change, clamped to 0..=100. Returns the new level.
pub fn change_volume(delta: i32) -> ControlResult<u32> {
    let current = get_volume()? as i32;
    set_volume((current + delta).clamp(0, 100) as u32)
}

pub fn is_muted() -> ControlResult<bool> {
    let _com = ComGuard::init();
    let volume = endpoint()?;
    // SAFETY: valid interface pointer.
    Ok(unsafe { volume.GetMute()? }.as_bool())
}

pub fn set_mute(muted: bool) -> ControlResult<()> {
    let _com = ComGuard::init();
    let volume = endpoint()?;
    // SAFETY: valid interface pointer; null event context.
    unsafe { volume.SetMute(muted, std::ptr::null())? };
    Ok(())
}

/// Sends a media command to the shell window (`WM_APPCOMMAND`); no synthetic key presses.
pub fn media_key(key: MediaKey) -> ControlResult<()> {
    // SAFETY: FindWindowW/PostMessageW with static strings and plain integers.
    unsafe {
        let shell = FindWindowW(w!("Shell_TrayWnd"), None)
            .map_err(|_| ControlError::unavailable("the taskbar window was not found"))?;
        PostMessageW(
            shell,
            WM_APPCOMMAND,
            WPARAM(shell.0 as usize),
            LPARAM(key.command() << 16),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_commands_match_winuser_values() {
        assert_eq!(MediaKey::Next.command(), 11);
        assert_eq!(MediaKey::Prev.command(), 12);
        assert_eq!(MediaKey::PlayPause.command(), 14);
        assert_eq!(MediaKey::PlayPause.as_str(), "play_pause");
    }
}
