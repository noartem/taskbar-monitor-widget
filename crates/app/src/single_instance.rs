//! One widget per user session.

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use windows::core::{HSTRING, PCWSTR};

use crate::app::{MAIN_CLASS, WM_APP_SHOW_SETTINGS};

const MUTEX_NAME: &str = "Local\\TaskbarMonitor.Instance";

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// `None` when another instance (possibly elevated) is already running.
pub fn acquire() -> Option<InstanceGuard> {
    unsafe {
        match CreateMutexW(None, true, &HSTRING::from(MUTEX_NAME)) {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    let _ = CloseHandle(h);
                    None
                } else {
                    Some(InstanceGuard(h))
                }
            }
            Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => None,
            Err(_) => None,
        }
    }
}

/// Asks the running instance to open its settings. Fails silently if it runs elevated (UIPI).
pub fn notify_existing() {
    unsafe {
        if let Ok(hwnd) = FindWindowW(&HSTRING::from(MAIN_CLASS), PCWSTR::null()) {
            let _ = PostMessageW(Some(hwnd), WM_APP_SHOW_SETTINGS, Default::default(), Default::default());
        }
    }
}
