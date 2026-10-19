//! Small Win32 helpers: wide strings, logging, message boxes, process launching.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW, SW_SHOWNORMAL,
};
use windows::core::{HSTRING, PCWSTR};

/// NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a NUL-terminated (or full) UTF-16 buffer.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Copies `s` into a fixed-size UTF-16 buffer, truncating and NUL-terminating.
pub fn copy_to_wide_buf(s: &str, buf: &mut [u16]) {
    let mut n = 0;
    for c in s.encode_utf16() {
        if n + 1 >= buf.len() {
            break;
        }
        buf[n] = c;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = 0;
    }
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("taskbar-monitor.exe"))
}

pub fn exe_dir() -> PathBuf {
    exe_path().parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

pub fn loword(v: u32) -> u16 {
    (v & 0xFFFF) as u16
}

/// Signed x/y from an LPARAM (GET_X_LPARAM / GET_Y_LPARAM).
pub fn point_from_lparam(lp: isize) -> (i32, i32) {
    ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32)
}

pub fn error_box(text: &str) {
    message_box(text, tm_core::i18n::tr("dlg.error"), MB_OK | MB_ICONERROR);
}

pub fn confirm(text: &str) -> bool {
    message_box(text, "Taskbar Monitor", MB_YESNO | MB_ICONQUESTION) == IDYES.0
}

fn message_box(text: &str, caption: &str, style: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) -> i32 {
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(caption), style | MB_TOPMOST | MB_SETFOREGROUND).0 }
}

/// Opens a file, folder or URL with its default handler.
pub fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn open_task_manager() {
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from("taskmgr.exe"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Starts another instance of this executable with `args` (no elevation).
pub fn spawn_self(args: &[&str]) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(exe_path()).args(args).creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */).spawn()
}

/// Joins arguments for a Windows command line, quoting where needed.
pub fn join_args(args: &[&str]) -> String {
    args.iter()
        .map(
            |a| {
                if a.is_empty() || a.contains([' ', '\t', '"']) {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.to_string()
                }
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn hwnd_from_isize(v: isize) -> HWND {
    HWND(v as *mut _)
}

/// Tiny append-only log with a size cap; useful for diagnosing sensor/taskbar issues on user machines.
pub struct Log {
    file: Mutex<Option<PathBuf>>,
}

pub static LOG: Log = Log { file: Mutex::new(None) };

impl Log {
    pub fn init(&self, path: PathBuf) {
        if std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
        if let Ok(mut f) = self.file.lock() {
            *f = Some(path);
        }
    }

    pub fn write(&self, msg: &str) {
        #[cfg(debug_assertions)]
        eprintln!("{msg}");
        let Ok(guard) = self.file.lock() else { return };
        let Some(path) = guard.as_ref() else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "[{}] {msg}", tm_core::time::now_unix());
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::util::LOG.write(&format!($($arg)*)) };
}
