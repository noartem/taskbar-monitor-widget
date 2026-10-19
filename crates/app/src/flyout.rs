//! Details popup: a card with a larger graph for every metric marked "show in details".

use std::collections::HashMap;

use tm_core::color::{Palette, Rgba};
use tm_core::config::{MetricConfig, Settings};
use tm_core::history::History;
use tm_core::i18n::{tr, trf};
use tm_core::metrics::{MetricCategory, MetricKind, MetricStyle};
use tm_core::resolver::Reading;
use tm_core::snapshot::Snapshot;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SetForegroundWindow, SetWindowPos,
    ShowWindow, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::HSTRING;

use crate::render::{Canvas, Gfx, Surface};
use crate::taskbar::{Edge, Taskbar};
use crate::tiles::{self, Rect, TileInput};

pub const FLYOUT_CLASS: &str = "TaskbarMonitor.Flyout";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyoutButton {
    Settings,
    Close,
}

pub struct FlyoutData<'a> {
    pub settings: &'a Settings,
    pub readings: &'a [Reading],
    pub histories: &'a HashMap<String, History>,
    pub snapshot: &'a Snapshot,
    pub palette: &'a Palette,
    pub elevated: bool,
    pub portable: bool,
}

pub struct Flyout {
    pub hwnd: HWND,
    surface: Surface,
    pub visible: bool,
    buttons: Vec<(Rect, FlyoutButton)>,
    anchor: Option<(Taskbar, RECT)>,
    pub hidden_at: Option<std::time::Instant>,
}

pub fn history_key(m: &MetricConfig) -> String {
    format!("{}|{}", m.kind.key(), m.source)
}

fn title(config: &MetricConfig, reading: &Reading) -> String {
    if !config.label.is_empty() {
        return config.label.clone();
    }
    match config.kind {
        MetricKind::ClaudeModel => format!("Claude · {} (7d)", reading.label),
        MetricKind::Sensor => reading.detail.clone().unwrap_or_else(|| reading.label.clone()),
        MetricKind::DiskSpace => format!("{} {}", tr("kind.disk_space"), reading.label),
        kind => tr(&format!("kind.{}", kind.key())).to_string(),
    }
}

impl Flyout {
    pub fn create(gfx: &Gfx) -> Option<Self> {
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                &HSTRING::from(FLYOUT_CLASS),
                &HSTRING::from("Taskbar Monitor"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(GetModuleHandleW(None).ok()?.into()),
                None,
            )
            .ok()?;
            Some(Self {
                hwnd,
                surface: Surface::new(gfx).ok()?,
                visible: false,
                buttons: Vec::new(),
                anchor: None,
                hidden_at: None,
            })
        }
    }

    pub fn show(&mut self, gfx: &Gfx, data: &FlyoutData, taskbar: Taskbar, anchor: RECT) {
        self.anchor = Some((taskbar, anchor));
        self.visible = true;
        self.refresh(gfx, data);
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
            self.hidden_at = Some(std::time::Instant::now());
        }
    }

    pub fn button_at(&self, x: i32, y: i32) -> Option<FlyoutButton> {
        self.buttons.iter().find(|(r, _)| r.contains(x as f32, y as f32)).map(|(_, b)| *b)
    }

    /// Redraws with fresh data (called on every snapshot while visible).
    pub fn refresh(&mut self, gfx: &Gfx, data: &FlyoutData) {
        let Some((taskbar, anchor)) = self.anchor.clone() else { return };
        if !self.visible {
            return;
        }
        let s = taskbar.scale;
        let p = data.palette;
        let shadow = (10.0 * s).round();
        let pad = (14.0 * s).round();
        let card_w = (236.0 * s).round();
        let card_h = (96.0 * s).round();
        let gap = (8.0 * s).round();
        let header_h = (36.0 * s).round();
        let footer_h = (24.0 * s).round();

        let items: Vec<usize> = data
            .settings
            .metrics
            .iter()
            .enumerate()
            .filter(|(i, m)| m.show_in_flyout && (data.readings[*i].available || !m.hide_when_unavailable))
            .map(|(i, _)| i)
            .collect();
        let cols = if items.len() <= 6 { 2 } else { 3 };
        let rows = items.len().div_ceil(cols).max(1);
        let content_w = cols as f32 * card_w + (cols - 1) as f32 * gap;
        let w = (content_w + 2.0 * pad + 2.0 * shadow).ceil() as i32;
        let h = (header_h + rows as f32 * card_h + (rows - 1) as f32 * gap + footer_h + 2.0 * pad + 2.0 * shadow).ceil() as i32;

        let family = data.settings.appearance.font_family.as_str();
        let (Ok(f_title), Ok(f_value), Ok(f_small), Ok(f_header), Ok(f_icon)) = (
            gfx.text_format(family, 12.0 * s, false),
            gfx.text_format(family, 20.0 * s, true),
            gfx.text_format(family, 11.0 * s, false),
            gfx.text_format(family, 14.0 * s, true),
            gfx.text_format("Segoe Fluent Icons, Segoe MDL2 Assets", 14.0 * s, false),
        ) else {
            return;
        };

        let mut buttons = Vec::new();
        let result = self.surface.draw(gfx, w, h, Rgba::TRANSPARENT, |c: &Canvas| {
            // Soft shadow.
            for i in 0..4 {
                let grow = shadow * (i as f32 + 1.0) / 4.0;
                c.fill_round(
                    shadow - grow,
                    shadow - grow + 2.0 * s,
                    w as f32 - 2.0 * (shadow - grow),
                    h as f32 - 2.0 * (shadow - grow),
                    10.0 * s + grow,
                    Rgba::BLACK.with_alpha(0.05),
                );
            }
            let (bx, by, bw, bh) = (shadow, shadow, w as f32 - 2.0 * shadow, h as f32 - 2.0 * shadow);
            c.fill_round(bx, by, bw, bh, 8.0 * s, p.surface.with_alpha(0.98));
            c.stroke_round(bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, 8.0 * s, p.surface_border, 1.0);

            // Header.
            let hx = bx + pad;
            let hy = by + pad;
            c.text_str("Taskbar Monitor", &f_header, hx, hy, p.foreground);
            let mode = format!(
                "{} · {}",
                if data.elevated { tr("flyout.admin") } else { tr("flyout.user") },
                if data.portable { tr("flyout.portable") } else { tr("flyout.installed") }
            );
            let icon_size = 24.0 * s;
            let close_x = bx + bw - pad - icon_size;
            let gear_x = close_x - icon_size - 4.0 * s;
            if let Ok(b) = gfx.layout(&mode, &f_small) {
                c.text(&b, gear_x - 8.0 * s - b.width, hy + 3.0 * s, p.label);
            }
            for (x, glyph, button) in [(gear_x, "\u{E713}", FlyoutButton::Settings), (close_x, "\u{E711}", FlyoutButton::Close)] {
                let r = Rect { x, y: hy - 2.0 * s, w: icon_size, h: icon_size };
                c.fill_round(r.x, r.y, r.w, r.h, 4.0 * s, p.foreground.with_alpha(0.06));
                if let Ok(b) = gfx.layout(glyph, &f_icon) {
                    c.text(&b, r.x + (r.w - b.width) / 2.0, r.y + (r.h - b.height) / 2.0, p.foreground);
                }
                buttons.push((r, button));
            }

            if items.is_empty() {
                c.text_str(tr("flyout.no_metrics"), &f_small, hx, hy + header_h, p.label);
            }

            // Cards.
            for (n, &i) in items.iter().enumerate() {
                let (col, row) = (n % cols, n / cols);
                let x = bx + pad + col as f32 * (card_w + gap);
                let y = by + pad + header_h + row as f32 * (card_h + gap);
                let config = &data.settings.metrics[i];
                let reading = &data.readings[i];
                let history = data.histories.get(&history_key(config));
                draw_card(
                    c,
                    gfx,
                    data,
                    config,
                    reading,
                    history,
                    Rect { x, y, w: card_w, h: card_h },
                    s,
                    (&f_title, &f_value, &f_small),
                );
            }

            // Footer: Claude account info.
            let fy = by + bh - pad - footer_h + 6.0 * s;
            let claude = &data.snapshot.claude;
            let mut footer = Vec::new();
            if let Some(plan) = &claude.subscription {
                footer.push(trf("flyout.plan", &[plan]));
            }
            if let Some(at) = claude.fetched_at {
                footer.push(trf("flyout.updated", &[&tm_core::format::duration(tm_core::time::now_unix() - at)]));
            }
            if claude.status != tm_core::claude::ClaudeStatus::Ok && claude.status != tm_core::claude::ClaudeStatus::Disabled {
                footer.push(format!("Claude: {}", tr(claude.status.key())));
            }
            if !footer.is_empty() {
                c.text_str(&footer.join("  ·  "), &f_small, hx, fy, p.label);
            }
        });
        if let Err(e) = result {
            crate::log!("flyout render failed: {e}");
            return;
        }
        self.buttons = buttons;

        // Place above (or beside) the taskbar, aligned with the widget.
        let m = taskbar.monitor_rect;
        let (mut x, mut y) = match taskbar.edge {
            Edge::Bottom => (anchor.right - w + shadow as i32, taskbar.rect.top - h + shadow as i32 / 2),
            Edge::Top => (anchor.right - w + shadow as i32, taskbar.rect.bottom - shadow as i32 / 2),
            Edge::Left => (taskbar.rect.right - shadow as i32 / 2, anchor.bottom - h + shadow as i32),
            Edge::Right => (taskbar.rect.left - w + shadow as i32 / 2, anchor.bottom - h + shadow as i32),
        };
        x = x.clamp(m.left, (m.right - w).max(m.left));
        y = y.clamp(m.top, (m.bottom - h).max(m.top));
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        }
        if let Err(e) = self.surface.present(self.hwnd, None) {
            crate::log!("flyout present failed: {e}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_card(
    c: &Canvas,
    gfx: &Gfx,
    data: &FlyoutData,
    config: &MetricConfig,
    reading: &Reading,
    history: Option<&History>,
    r: Rect,
    s: f32,
    fonts: (
        &windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
        &windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
        &windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    ),
) {
    let (f_title, f_value, f_small) = fonts;
    let p = data.palette;
    let a = &data.settings.appearance;
    let pad = 10.0 * s;
    c.fill_round(r.x, r.y, r.w, r.h, 6.0 * s, p.foreground.with_alpha(if p.dark { 0.05 } else { 0.04 }));

    let base = tiles::base_color(config, p);
    let color = if reading.available && reading.uses_thresholds { p.threshold(base, reading.value, a) } else { base };
    c.fill_round(r.x + pad, r.y + pad + 3.0 * s, 3.0 * s, 10.0 * s, 1.5 * s, color);

    let value_color = if !reading.available {
        p.label
    } else if reading.uses_thresholds {
        p.threshold(p.foreground, reading.value, a)
    } else {
        p.foreground
    };
    let value_w = match gfx.layout(&reading.value_text, f_value) {
        Ok(b) => {
            c.text(&b, r.x + r.w - pad - b.width, r.y + pad - 3.0 * s, value_color);
            b.width
        }
        Err(_) => 0.0,
    };
    let title_text = title(config, reading);
    c.clip(r.x, r.y, r.w - pad - value_w - 6.0 * s, r.h);
    c.text_str(&title_text, f_title, r.x + pad + 8.0 * s, r.y + pad, p.foreground);
    c.unclip();

    // Detail line.
    let detail = if let Some(status) = reading.status.filter(|_| !reading.available) {
        tr(status).to_string()
    } else {
        let mut parts = Vec::new();
        if let Some(secs) = reading.resets_in {
            parts.push(trf("flyout.resets", &[&tm_core::format::duration(secs)]));
        } else if let Some(d) = &reading.detail {
            parts.push(d.clone());
        }
        // The title already names Claude limits and drives.
        if let Some(src) = &reading.source_name
            && config.kind.def().category != MetricCategory::Claude
            && config.kind != MetricKind::DiskSpace
            && !parts.iter().any(|p| p.contains(src.as_str()))
        {
            parts.push(src.clone());
        }
        if let Some(status) = reading.status {
            parts.push(tr(status).to_string());
        }
        parts.join(" · ")
    };
    c.clip(r.x, r.y, r.w - pad, r.h);
    c.text_str(&detail, f_small, r.x + pad, r.y + pad + 22.0 * s, p.label);
    c.unclip();

    // Visual: bar for limits/capacity, graph for everything that changes quickly.
    let gx = r.x + pad;
    let gw = r.w - 2.0 * pad;
    let gy = r.y + pad + 42.0 * s;
    let gh = r.y + r.h - pad - gy;
    let input = TileInput { config, reading, history };
    let bar_like = matches!(config.kind.def().category, MetricCategory::Claude) || config.kind == MetricKind::DiskSpace;
    if bar_like {
        let bh = 6.0 * s;
        let by = gy + (gh - bh) / 2.0;
        c.fill_round(gx, by, gw, bh, bh / 2.0, p.track);
        if reading.available {
            let fw = gw * tiles::fraction(&input) as f32;
            c.fill_round(gx, by, if fw > 0.0 { fw.max(bh) } else { 0.0 }, bh, bh / 2.0, color);
        }
    } else {
        c.fill_round(gx, gy, gw, gh, 3.0 * s, p.track.mul_alpha(0.5));
        if let Some(h) = history {
            let max = tiles::graph_max(config, reading, Some(h));
            let style = if config.style == MetricStyle::Columns { MetricStyle::Columns } else { MetricStyle::Area };
            tiles::draw_graph(c, h, style, gx, gy, gw, gh, max, color, s);
        }
    }
}

impl Drop for Flyout {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
