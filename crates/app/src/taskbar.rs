//! Locating taskbars, their notification areas, and detecting full-screen apps.

use tm_core::config::{Layout, MonitorMode, Placement};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowRect, IsWindowVisible,
    MONITORINFOF_PRIMARY,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct Taskbar {
    pub hwnd: HWND,
    pub rect: RECT,
    pub monitor: HMONITOR,
    pub monitor_rect: RECT,
    pub edge: Edge,
    pub primary: bool,
    /// Notification area (primary taskbar only).
    pub tray: Option<RECT>,
    pub scale: f32,
    /// Auto-hidden taskbar currently slid off screen.
    pub hidden: bool,
}

impl Taskbar {
    pub fn horizontal(&self) -> bool {
        matches!(self.edge, Edge::Bottom | Edge::Top)
    }

    pub fn thickness(&self) -> i32 {
        if self.horizontal() { self.rect.bottom - self.rect.top } else { self.rect.right - self.rect.left }
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

unsafe extern "system" fn collect_secondary(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    if class_name(hwnd) == "Shell_SecondaryTrayWnd" {
        list.push(hwnd);
    }
    BOOL(1)
}

fn info_for(hwnd: HWND) -> Option<Taskbar> {
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
            return None;
        }
        let m = mi.rcMonitor;
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let edge = if w >= h {
            if (rect.top + rect.bottom) / 2 < (m.top + m.bottom) / 2 { Edge::Top } else { Edge::Bottom }
        } else if (rect.left + rect.right) / 2 < (m.left + m.right) / 2 {
            Edge::Left
        } else {
            Edge::Right
        };
        // Visible thickness on its monitor: an auto-hidden taskbar keeps only a couple of pixels on screen.
        let visible = match edge {
            Edge::Bottom => m.bottom - rect.top.max(m.top),
            Edge::Top => rect.bottom.min(m.bottom) - m.top,
            Edge::Left => rect.right.min(m.right) - m.left,
            Edge::Right => m.right - rect.left.max(m.left),
        };
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);

        let tray = FindWindowExW(Some(hwnd), None, &HSTRING::from("TrayNotifyWnd"), PCWSTR::null()).ok().and_then(|t| {
            let mut r = RECT::default();
            GetWindowRect(t, &mut r).ok()?;
            (r.right > r.left && r.bottom > r.top && r.left >= rect.left - 2 && r.right <= rect.right + 2).then_some(r)
        });

        Some(Taskbar {
            hwnd,
            rect,
            monitor,
            monitor_rect: m,
            edge,
            primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
            tray,
            scale: dpi_x as f32 / 96.0,
            hidden: visible < 6 || !IsWindowVisible(hwnd).as_bool(),
        })
    }
}

/// Re-reads geometry of a known taskbar (cheaper than [`enumerate`]).
pub fn refresh(tb: &Taskbar) -> Option<Taskbar> {
    info_for(tb.hwnd).map(|mut t| {
        t.primary = tb.primary;
        t
    })
}

/// All taskbars, primary first.
pub fn enumerate() -> Vec<Taskbar> {
    let mut result = Vec::new();
    unsafe {
        if let Ok(primary) = FindWindowW(&HSTRING::from("Shell_TrayWnd"), PCWSTR::null())
            && let Some(t) = info_for(primary)
        {
            result.push(t);
        }
        let mut secondary: Vec<HWND> = Vec::new();
        let _ = EnumWindows(Some(collect_secondary), LPARAM(&mut secondary as *mut _ as isize));
        result.extend(secondary.into_iter().filter_map(info_for));
    }
    if let Some(first) = result.first_mut() {
        first.primary = true;
    }
    result
}

/// Taskbars the widget should appear on.
pub fn select(all: Vec<Taskbar>, layout: &Layout) -> Vec<Taskbar> {
    match layout.monitor {
        MonitorMode::Primary => all.into_iter().take(1).collect(),
        MonitorMode::All => all,
        MonitorMode::Specific if all.is_empty() => all,
        MonitorMode::Specific => {
            let idx = layout.monitor_index as usize;
            let mut sorted = all;
            // Primary stays 0; others ordered left-to-right, top-to-bottom.
            sorted[1..].sort_by_key(|t| (t.monitor_rect.left, t.monitor_rect.top));
            let chosen = if idx < sorted.len() { sorted.swap_remove(idx) } else { sorted.swap_remove(0) };
            vec![chosen]
        }
    }
}

/// Screen position (top-left, physical pixels) for a widget of `size` on `tb`.
pub fn position(tb: &Taskbar, size: (i32, i32), layout: &Layout) -> (i32, i32) {
    let s = tb.scale;
    let (w, h) = size;
    let off_x = (layout.offset_x * s).round() as i32;
    let off_y = (layout.offset_y * s).round() as i32;
    let r = tb.rect;
    let gap = (6.0 * s) as i32;
    if tb.horizontal() {
        let y = r.top + ((r.bottom - r.top) - h) / 2 + off_y;
        let tray_left = match tb.tray {
            Some(t) => t.left,
            None if tb.primary => r.right - (280.0 * s) as i32,
            None => r.right - (layout.secondary_right_margin * s) as i32,
        };
        let x = match layout.placement {
            Placement::NearTray => tray_left - gap - w - off_x,
            Placement::Left => r.left + gap + off_x,
            Placement::Center => r.left + ((r.right - r.left) - w) / 2 + off_x,
        };
        (x.clamp(r.left, (r.right - w).max(r.left)), y)
    } else {
        let x = r.left + ((r.right - r.left) - w) / 2 + off_y;
        let tray_top = match tb.tray {
            Some(t) => t.top,
            None => r.bottom - (200.0 * s) as i32,
        };
        let y = match layout.placement {
            Placement::NearTray => tray_top - gap - h - off_x,
            Placement::Left => r.top + gap + off_x,
            Placement::Center => r.top + ((r.bottom - r.top) - h) / 2 + off_x,
        };
        (x, y.clamp(r.top, (r.bottom - h).max(r.top)))
    }
}

/// True when a full-screen app (game, video, presentation) covers `monitor`.
pub fn fullscreen_on(monitor: HMONITOR, monitor_rect: RECT) -> bool {
    unsafe {
        if let Ok(state) = SHQueryUserNotificationState()
            && (state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE)
        {
            // Reported globally; only trust it for the monitor the foreground window is on.
            let fg = GetForegroundWindow();
            if !fg.is_invalid() && MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) == monitor {
                return true;
            }
        }
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let class = class_name(fg);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "#32769") {
            return false;
        }
        if MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) != monitor {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        r.left <= monitor_rect.left
            && r.top <= monitor_rect.top
            && r.right >= monitor_rect.right
            && r.bottom >= monitor_rect.bottom
    }
}
