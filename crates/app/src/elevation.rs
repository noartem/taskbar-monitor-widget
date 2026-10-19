//! Administrator rights: detection and relaunching through UAC.

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcess, OpenProcessToken, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};

use crate::util;

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

pub enum ElevatedRun {
    /// The user declined the UAC prompt.
    Cancelled,
    Started,
    Finished(u32),
}

/// Starts this executable as administrator. With `wait`, blocks until it exits and returns its exit code.
pub fn run_self_elevated(args: &[&str], wait: bool) -> windows::core::Result<ElevatedRun> {
    let file = HSTRING::from(util::exe_path().as_os_str());
    let params = HSTRING::from(util::join_args(args));
    let dir = HSTRING::from(util::exe_dir().as_os_str());
    let verb = HSTRING::from("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        lpDirectory: PCWSTR(dir.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Ok(ElevatedRun::Cancelled);
            }
            return Err(e);
        }
        if info.hProcess.is_invalid() {
            return Ok(ElevatedRun::Started);
        }
        let result = if wait {
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 0u32;
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            ElevatedRun::Finished(code)
        } else {
            ElevatedRun::Started
        };
        let _ = CloseHandle(info.hProcess);
        Ok(result)
    }
}

/// Waits (bounded) for a previous instance to exit before this one takes over.
pub fn wait_for_process_exit(pid: u32, timeout_ms: u32) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let r = WaitForSingleObject(h, timeout_ms);
            if r != WAIT_OBJECT_0 {
                crate::log!("previous instance {pid} did not exit in time");
            }
            let _ = CloseHandle(h);
        }
    }
}
