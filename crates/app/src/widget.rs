//! A widget window bound to one taskbar.

use tm_core::color::{Palette, Rgba};
use tm_core::config::{EmbedMode, Settings};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_SETMAXTIPWIDTH, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP,
    TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DestroyWindow, GWL_STYLE, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, PWSTR};

use crate::render::{Gfx, Surface};
use crate::taskbar::{self, Taskbar};
use crate::tiles::{self, Avail, TileInput, WidgetLayout};

pub const WIDGET_CLASS: &str = "TaskbarMonitor.Widget";

pub struct Widget {
    pub hwnd: HWND,
    pub taskbar: Taskbar,
    surface: Surface,
    pub layout: Option<WidgetLayout>,
    pub size: (i32, i32),
    pub pos: (i32, i32),
    pub visible: bool,
    pub embedded: bool,
    tooltip: HWND,
    tooltip_text: String,
    pub hover: Option<usize>,
    /// A new frame was drawn but not yet pushed to the window.
    dirty: bool,
    /// Last applied (x, y, w, h) in the window's coordinate space.
    applied: Option<(i32, i32, i32, i32)>,
}

impl Widget {
    pub fn create(taskbar: Taskbar, embed: EmbedMode, gfx: &Gfx) -> Option<Self> {
        let embedded = embed == EmbedMode::Embedded;
        unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            let ex =
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | if embedded { WINDOW_EX_STYLE(0) } else { WS_EX_TOPMOST };
            let hwnd = CreateWindowExW(
                ex,
                &HSTRING::from(WIDGET_CLASS),
                &HSTRING::from("Taskbar Monitor"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            if embedded {
                SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_CHILD | WS_CLIPSIBLINGS).0 as isize);
                if SetParent(hwnd, Some(taskbar.hwnd)).is_err() {
                    crate::log!("embedding into the taskbar failed; using overlay");
                    SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP.0 as isize);
                }
            }

            let tooltip = CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                None,
                WS_POPUP | WINDOW_STYLE(TTS_NOPREFIX | TTS_ALWAYSTIP),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(hwnd),
                None,
                Some(instance.into()),
                None,
            )
            .unwrap_or_default();
            if !tooltip.is_invalid() {
                let mut empty = [0u16; 1];
                let info = TTTOOLINFOW {
                    cbSize: size_of::<TTTOOLINFOW>() as u32,
                    uFlags: TTF_SUBCLASS,
                    hwnd,
                    uId: 1,
                    lpszText: PWSTR(empty.as_mut_ptr()),
                    ..Default::default()
                };
                SendMessageW(tooltip, TTM_ADDTOOLW, None, Some(LPARAM(&info as *const _ as isize)));
                SendMessageW(tooltip, TTM_SETMAXTIPWIDTH, None, Some(LPARAM(480)));
            }

            Some(Self {
                hwnd,
                taskbar,
                surface: Surface::new(gfx).ok()?,
                layout: None,
                size: (0, 0),
                pos: (0, 0),
                visible: false,
                embedded,
                tooltip,
                tooltip_text: String::new(),
                hover: None,
                dirty: false,
                applied: None,
            })
        }
    }

    pub fn render(&mut self, gfx: &Gfx, inputs: &[TileInput], settings: &Settings, palette: &Palette) {
        let avail =
            Avail { horizontal: self.taskbar.horizontal(), thickness: self.taskbar.thickness(), scale: self.taskbar.scale };
        let layout = tiles::layout(gfx, inputs, settings, avail);
        let (w, h) = (layout.width, layout.height);
        let radius = settings.appearance.corner_radius * self.taskbar.scale;
        // Alpha 1/255 everywhere keeps the whole rectangle clickable while staying invisible.
        let clear = Rgba::new(0.0, 0.0, 0.0, 1.0 / 255.0);
        let result = self.surface.draw(gfx, w, h, clear, |c| {
            if palette.background.a > 0.0 {
                c.fill_round(0.0, 0.0, w as f32, h as f32, radius, palette.background);
            }
            tiles::draw(c, &layout, inputs, settings, palette);
        });
        if let Err(e) = result {
            crate::log!("widget render failed: {e}");
        }
        if (w, h) != self.size && !self.tooltip.is_invalid() {
            let info = TTTOOLINFOW {
                cbSize: size_of::<TTTOOLINFOW>() as u32,
                hwnd: self.hwnd,
                uId: 1,
                rect: RECT { left: 0, top: 0, right: w, bottom: h },
                ..Default::default()
            };
            unsafe { SendMessageW(self.tooltip, TTM_NEWTOOLRECTW, None, Some(LPARAM(&info as *const _ as isize))) };
        }
        self.size = (w, h);
        self.layout = Some(layout);
        self.dirty = true;
    }

    /// Moves the window next to its taskbar's tray (or configured anchor) and pushes the latest frame.
    /// Does nothing when neither the position nor the content changed.
    pub fn place(&mut self, settings: &Settings, show: bool) {
        let show = show && self.layout.as_ref().is_some_and(|l| !l.tiles.is_empty());
        if !show {
            if self.visible {
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                self.visible = false;
            }
            return;
        }
        let (x, y) = taskbar::position(&self.taskbar, self.size, &settings.layout);
        self.pos = (x, y);
        let (mut cx, mut cy) = (x, y);
        if self.embedded {
            let mut pt = POINT { x, y };
            unsafe {
                let _ = ScreenToClient(self.taskbar.hwnd, &mut pt);
            }
            (cx, cy) = (pt.x, pt.y);
        }
        let geometry = (cx, cy, self.size.0, self.size.1);
        if self.visible && !self.dirty && self.applied == Some(geometry) {
            return;
        }
        unsafe {
            if self.applied != Some(geometry) || !self.visible {
                let insert_after = if self.embedded { None } else { Some(HWND_TOPMOST) };
                let flags = SWP_NOACTIVATE | if self.embedded { SWP_NOZORDER } else { Default::default() };
                let _ = SetWindowPos(self.hwnd, insert_after, cx, cy, self.size.0, self.size.1, flags);
                self.applied = Some(geometry);
            }
            if let Err(e) = self.surface.present(self.hwnd, None) {
                crate::log!("UpdateLayeredWindow failed: {e}");
            }
            self.dirty = false;
            if !self.visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
            }
        }
    }

    /// Re-asserts top-most z-order (the taskbar raises itself when clicked).
    pub fn raise(&self) {
        if self.visible && !self.embedded {
            unsafe {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// Index of the tile input under client point (x, y).
    pub fn hit(&self, x: i32, y: i32) -> Option<usize> {
        self.layout.as_ref()?.tiles.iter().find(|t| t.rect.contains(x as f32, y as f32)).map(|t| t.input)
    }

    pub fn set_tooltip(&mut self, text: &str) {
        if self.tooltip.is_invalid() || text == self.tooltip_text {
            return;
        }
        self.tooltip_text = text.to_string();
        let mut wide = crate::util::wide(text);
        let info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            hwnd: self.hwnd,
            uId: 1,
            lpszText: PWSTR(wide.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { SendMessageW(self.tooltip, TTM_UPDATETIPTEXTW, Some(WPARAM(0)), Some(LPARAM(&info as *const _ as isize))) };
    }

    pub fn screen_rect(&self) -> RECT {
        RECT { left: self.pos.0, top: self.pos.1, right: self.pos.0 + self.size.0, bottom: self.pos.1 + self.size.1 }
    }
}

impl Drop for Widget {
    fn drop(&mut self) {
        unsafe {
            if !self.tooltip.is_invalid() {
                let _ = DestroyWindow(self.tooltip);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
