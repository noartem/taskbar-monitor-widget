//! Notification area icon: tooltip, context menu and an optional live metric icon.

use tm_core::color::{Palette, Rgba};
use tm_core::config::{Autostart, Settings};
use tm_core::i18n::tr;
use tm_core::presets;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, HMENU,
    ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW,
    SM_CXSMICON, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_NULL,
};
use windows::core::{HSTRING, PCWSTR};

use crate::app::WM_APP_TRAY;
use crate::render::{Gfx, Surface};
use crate::util;

pub const CMD_DETAILS: u32 = 1;
pub const CMD_SETTINGS: u32 = 2;
pub const CMD_TOGGLE_WIDGET: u32 = 3;
pub const CMD_LOCK: u32 = 4;
pub const CMD_PRESET_BASE: u32 = 100;
pub const CMD_ROWS_BASE: u32 = 200;
pub const CMD_AUTOSTART_OFF: u32 = 300;
pub const CMD_AUTOSTART_USER: u32 = 301;
pub const CMD_AUTOSTART_ELEVATED: u32 = 302;
pub const CMD_RESTART_ADMIN: u32 = 400;
pub const CMD_REFRESH_CLAUDE: u32 = 401;
pub const CMD_OPEN_CONFIG: u32 = 402;
pub const CMD_OPEN_FOLDER: u32 = 403;
pub const CMD_EXIT: u32 = 500;

pub struct Tray {
    hwnd: HWND,
    app_icon: HICON,
    dynamic_icon: Option<HICON>,
    added: bool,
    last_tip: String,
    surface: Option<Surface>,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        let app_icon = unsafe {
            let size = GetSystemMetrics(SM_CXSMICON);
            LoadImageW(
                GetModuleHandleW(None).ok().map(|h| h.into()),
                PCWSTR(std::ptr::dangling::<u16>()), // icon resource #1
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
            .map(|h| HICON(h.0))
            .unwrap_or_default()
        };
        Self { hwnd, app_icon, dynamic_icon: None, added: false, last_tip: String::new(), surface: None }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uCallbackMessage: WM_APP_TRAY,
            ..Default::default()
        }
    }

    fn icon(&self) -> HICON {
        self.dynamic_icon.unwrap_or(self.app_icon)
    }

    /// (Re)adds the icon, e.g. after Explorer restarted.
    pub fn add(&mut self) {
        let mut d = self.data();
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        d.hIcon = self.icon();
        util::copy_to_wide_buf(if self.last_tip.is_empty() { "Taskbar Monitor" } else { &self.last_tip }, &mut d.szTip);
        unsafe {
            self.added = Shell_NotifyIconW(NIM_ADD, &d).as_bool();
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
        }
    }

    pub fn remove(&mut self) {
        if self.added {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            }
            self.added = false;
        }
    }

    pub fn set_tooltip(&mut self, tip: &str) {
        if tip == self.last_tip || !self.added {
            return;
        }
        self.last_tip = tip.to_string();
        let mut d = self.data();
        d.uFlags = NIF_TIP | NIF_SHOWTIP;
        util::copy_to_wide_buf(tip, &mut d.szTip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    /// Draws `text` with a level bar into the tray icon; `None` restores the application icon.
    pub fn set_metric_icon(&mut self, gfx: &Gfx, content: Option<(&str, f32, Rgba)>, palette: &Palette, scale: f32) {
        let new_icon = content.and_then(|(text, fraction, color)| self.render_icon(gfx, text, fraction, color, palette, scale));
        if let Some(old) = std::mem::replace(&mut self.dynamic_icon, new_icon) {
            unsafe {
                let _ = DestroyIcon(old);
            }
        }
        if self.added {
            let mut d = self.data();
            d.uFlags = NIF_ICON;
            d.hIcon = self.icon();
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
            }
        }
    }

    fn render_icon(&mut self, gfx: &Gfx, text: &str, fraction: f32, color: Rgba, palette: &Palette, scale: f32) -> Option<HICON> {
        let size = ((unsafe { GetSystemMetrics(SM_CXSMICON) }) as f32).max(16.0 * scale).round() as i32;
        if self.surface.is_none() {
            self.surface = Surface::new(gfx).ok();
        }
        let surface = self.surface.as_mut()?;
        let s = size as f32;
        let font_size = if text.chars().count() >= 3 { s * 0.5 } else { s * 0.66 };
        let format = gfx.text_format("Segoe UI", font_size, true).ok()?;
        surface
            .draw(gfx, size, size, Rgba::TRANSPARENT, |c| {
                let bar_h = (s * 0.16).max(2.0);
                c.fill_rect(0.0, s - bar_h, s, bar_h, palette.track);
                c.fill_rect(0.0, s - bar_h, s * fraction.clamp(0.0, 1.0), bar_h, color);
                if let Ok(b) = gfx.layout(text, &format) {
                    c.text(&b, (s - b.width) / 2.0, (s - bar_h - b.height) / 2.0, palette.foreground);
                }
            })
            .ok()?;
        icon_from_premultiplied(surface.pixels(), size, size)
    }

    /// Shows the context menu at the cursor and returns the chosen command (0 = dismissed).
    pub fn show_menu(&self, settings: &Settings, widget_visible: bool, elevated: bool, autostart: Autostart) -> u32 {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return 0 };
            let add = |m: HMENU, id: u32, text: &str, checked: bool| {
                let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
                let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(text));
            };
            let sep = |m: HMENU| {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            };

            add(menu, CMD_DETAILS, tr("menu.details"), false);
            add(menu, CMD_SETTINGS, tr("menu.settings"), false);
            sep(menu);
            add(menu, CMD_TOGGLE_WIDGET, tr("menu.show_widget"), widget_visible);
            add(menu, CMD_LOCK, tr("menu.lock"), settings.layout.lock_position);

            if let Ok(layouts) = CreatePopupMenu() {
                for (i, p) in presets::ALL.iter().enumerate() {
                    add(layouts, CMD_PRESET_BASE + i as u32, tr(&format!("preset.{}", p.key)), false);
                }
                sep(layouts);
                for rows in 1..=4u32 {
                    add(layouts, CMD_ROWS_BASE + rows, &format!("{}: {rows}", tr("menu.rows")), settings.layout.rows == rows);
                }
                let _ = AppendMenuW(menu, MF_POPUP, layouts.0 as usize, &HSTRING::from(tr("menu.layout")));
            }
            if let Ok(auto) = CreatePopupMenu() {
                add(auto, CMD_AUTOSTART_OFF, tr("autostart.off"), autostart == Autostart::Off);
                add(auto, CMD_AUTOSTART_USER, tr("autostart.user"), autostart == Autostart::User);
                add(auto, CMD_AUTOSTART_ELEVATED, tr("autostart.elevated"), autostart == Autostart::Elevated);
                let _ = AppendMenuW(menu, MF_POPUP, auto.0 as usize, &HSTRING::from(tr("menu.autostart")));
            }
            sep(menu);
            if elevated {
                let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED | MF_CHECKED, 0, &HSTRING::from(tr("menu.running_admin")));
            } else {
                add(menu, CMD_RESTART_ADMIN, tr("menu.restart_admin"), false);
            }
            if settings.claude.enabled {
                add(menu, CMD_REFRESH_CLAUDE, tr("menu.refresh_claude"), false);
            }
            add(menu, CMD_OPEN_CONFIG, tr("menu.open_config"), false);
            add(menu, CMD_OPEN_FOLDER, tr("menu.open_folder"), false);
            sep(menu);
            add(menu, CMD_EXIT, tr("menu.exit"), false);

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Required so the menu closes when the user clicks elsewhere.
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0, pt.x, pt.y, self.hwnd, None);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, Default::default(), Default::default());
            let _ = DestroyMenu(menu);
            cmd.0 as u32
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        if let Some(i) = self.dynamic_icon.take() {
            unsafe {
                let _ = DestroyIcon(i);
            }
        }
    }
}

/// Builds an HICON from premultiplied BGRA pixels (icons expect straight alpha).
pub fn icon_from_premultiplied(pixels: &[u8], width: i32, height: i32) -> Option<HICON> {
    if pixels.len() < (width * height * 4) as usize {
        return None;
    }
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color: HBITMAP = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, pixels.len());
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pixels.as_chunks::<4>().0) {
            let a = s[3];
            if a == 0 {
                d.copy_from_slice(&[0, 0, 0, 0]);
            } else {
                let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                d.copy_from_slice(&[un(s[0]), un(s[1]), un(s[2]), a]);
            }
        }
        let mask = CreateBitmap(width, height, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.ok()
    }
}
