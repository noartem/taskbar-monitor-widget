//! Direct2D/DirectWrite drawing into per-pixel-alpha layered windows.
//!
//! A software DC render target draws into a 32-bit DIB which is pushed to the window with
//! `UpdateLayeredWindow`. Software rendering avoids creating a GPU device: for a few hundred pixels redrawn
//! once per second it is cheaper in both memory and CPU.

use std::cell::RefCell;
use std::collections::HashMap;

use tm_core::color::Rgba;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_CAP_STYLE_ROUND,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory,
    IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{HSTRING, Result};
use windows_numerics::Vector2;

fn color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct FormatKey {
    family: String,
    size_centi: u32,
    bold: bool,
}

/// Device-independent drawing resources shared by all windows (single UI thread).
pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    round_stroke: ID2D1StrokeStyle,
    formats: RefCell<HashMap<FormatKey, IDWriteTextFormat>>,
    fonts: Option<IDWriteFontCollection>,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let round_stroke = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    ..Default::default()
                },
                None,
            )?;
            let mut fonts = None;
            let _ = dwrite.GetSystemFontCollection(&mut fonts, false);
            Ok(Self { d2d, dwrite, round_stroke, formats: RefCell::new(HashMap::new()), fonts })
        }
    }

    /// First installed family from a comma separated list, then common UI fonts, then any installed family.
    fn resolve_family(&self, families: &str) -> String {
        let Some(fonts) = &self.fonts else { return "Segoe UI".into() };
        let requested = families.split(',').map(str::trim).filter(|s| !s.is_empty());
        for name in requested.chain(["Segoe UI", "Tahoma", "Arial"]) {
            let mut index = 0u32;
            let mut exists = windows::core::BOOL(0);
            if unsafe { fonts.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists) }.is_ok() && exists.as_bool() {
                return name.to_string();
            }
        }
        unsafe {
            fonts
                .GetFontFamily(0)
                .and_then(|f| f.GetFamilyNames())
                .and_then(|names| {
                    let len = names.GetStringLength(0)? as usize;
                    let mut buf = vec![0u16; len + 1];
                    names.GetString(0, &mut buf)?;
                    Ok(crate::util::from_wide(&buf))
                })
                .unwrap_or_else(|_| "Segoe UI".into())
        }
    }

    pub fn text_format(&self, family: &str, size: f32, bold: bool) -> Result<IDWriteTextFormat> {
        let key = FormatKey { family: family.to_string(), size_centi: (size * 100.0) as u32, bold };
        if let Some(f) = self.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let resolved = self.resolve_family(family);
        let format = unsafe {
            let f = self.dwrite.CreateTextFormat(
                &HSTRING::from(resolved),
                None,
                if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size.max(1.0),
                &HSTRING::from("en-us"),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f
        };
        let mut formats = self.formats.borrow_mut();
        if formats.len() > 64 {
            formats.clear();
        }
        formats.insert(key, format.clone());
        Ok(format)
    }

    pub fn layout(&self, text: &str, format: &IDWriteTextFormat) -> Result<TextBlock> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&wide, format, 10_000.0, 10_000.0)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok(TextBlock { layout, width: m.widthIncludingTrailingWhitespace, height: m.height })
        }
    }

    /// Width and height of `text`; (0, 0) on failure.
    pub fn measure(&self, text: &str, format: &IDWriteTextFormat) -> (f32, f32) {
        self.layout(text, format).map(|t| (t.width, t.height)).unwrap_or((0.0, 0.0))
    }
}

pub struct TextBlock {
    pub layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}

/// An offscreen 32-bit premultiplied BGRA bitmap with a Direct2D render target bound to it.
pub struct Surface {
    hdc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
    rt: ID2D1DCRenderTarget,
}

impl Surface {
    pub fn new(gfx: &Gfx) -> Result<Self> {
        let rt = Self::create_target(gfx)?;
        let hdc = unsafe { CreateCompatibleDC(None) };
        Ok(Self { hdc, bitmap: HBITMAP::default(), old: HGDIOBJ::default(), bits: std::ptr::null_mut(), width: 0, height: 0, rt })
    }

    fn create_target(gfx: &Gfx) -> Result<ID2D1DCRenderTarget> {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        unsafe {
            let rt = gfx.d2d.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            Ok(rt)
        }
    }

    fn ensure_size(&mut self, width: i32, height: i32) -> Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        if width == self.width && height == self.height && !self.bits.is_null() {
            return Ok(());
        }
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.hdc, self.old);
                let _ = DeleteObject(self.bitmap.into());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            self.bitmap = CreateDIBSection(Some(self.hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            self.bits = bits as *mut u8;
            self.old = SelectObject(self.hdc, self.bitmap.into());
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Clears to `clear` and runs `draw`. Coordinates are physical pixels.
    pub fn draw(&mut self, gfx: &Gfx, width: i32, height: i32, clear: Rgba, draw: impl FnOnce(&Canvas)) -> Result<()> {
        self.ensure_size(width, height)?;
        let rect = RECT { left: 0, top: 0, right: self.width, bottom: self.height };
        unsafe {
            self.rt.BindDC(self.hdc, &rect)?;
            self.rt.BeginDraw();
            self.rt.Clear(Some(&color(clear)));
            let brush = self.rt.CreateSolidColorBrush(&color(Rgba::WHITE), None)?;
            draw(&Canvas { rt: &self.rt, gfx, brush, round: &gfx.round_stroke });
            if let Err(e) = self.rt.EndDraw(None, None) {
                // D2DERR_RECREATE_TARGET: rebuild the render target and try again next frame.
                self.rt = Self::create_target(gfx)?;
                return Err(e);
            }
        }
        Ok(())
    }

    /// Pushes the bitmap to a layered window. `pos` = new screen (or parent client) position, if any.
    pub fn present(&self, hwnd: HWND, pos: Option<POINT>) -> Result<()> {
        let size = SIZE { cx: self.width, cy: self.height };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                pos.as_ref().map(|p| p as *const POINT),
                Some(&size),
                Some(self.hdc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }

    /// Premultiplied BGRA pixels, row-major top-down.
    pub fn pixels(&self) -> &[u8] {
        if self.bits.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.bits, (self.width * self.height * 4) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.hdc, self.old);
                let _ = DeleteObject(self.bitmap.into());
            }
            let _ = DeleteDC(self.hdc);
        }
    }
}

/// Drawing API used by tiles, the flyout and the tray icon.
pub struct Canvas<'a> {
    rt: &'a ID2D1DCRenderTarget,
    pub gfx: &'a Gfx,
    brush: ID2D1SolidColorBrush,
    round: &'a ID2D1StrokeStyle,
}

impl Canvas<'_> {
    fn brush(&self, c: Rgba) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&color(c)) };
        &self.brush
    }

    pub fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, c: Rgba) {
        if w <= 0.0 || h <= 0.0 || c.a <= 0.0 {
            return;
        }
        unsafe { self.rt.FillRectangle(&D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, self.brush(c)) };
    }

    pub fn fill_round(&self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
        if w <= 0.0 || h <= 0.0 || c.a <= 0.0 {
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r };
        unsafe { self.rt.FillRoundedRectangle(&rr, self.brush(c)) };
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round(&self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba, width: f32) {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.brush(c), width, None) };
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, r: f32, c: Rgba, width: f32) {
        let e = D2D1_ELLIPSE { point: v2(cx, cy), radiusX: r, radiusY: r };
        unsafe { self.rt.DrawEllipse(&e, self.brush(c), width, None) };
    }

    /// Clockwise arc starting at 12 o'clock covering `fraction` (0..1) of the circle.
    pub fn arc(&self, cx: f32, cy: f32, r: f32, fraction: f32, c: Rgba, width: f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction <= 0.001 {
            return;
        }
        if fraction >= 0.999 {
            self.stroke_circle(cx, cy, r, c, width);
            return;
        }
        let angle = fraction * std::f32::consts::TAU;
        let end = v2(cx + r * angle.sin(), cy - r * angle.cos());
        let result: Result<()> = (|| unsafe {
            let path = self.gfx.d2d.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.BeginFigure(v2(cx, cy - r), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: end,
                size: D2D_SIZE_F { width: r, height: r },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            self.rt.DrawGeometry(&path, self.brush(c), width, self.round);
            Ok(())
        })();
        let _ = result;
    }

    pub fn polyline(&self, points: &[(f32, f32)], c: Rgba, width: f32) {
        self.path(points, false, c, width);
    }

    /// Filled polygon (closed automatically).
    pub fn fill_polygon(&self, points: &[(f32, f32)], c: Rgba) {
        self.path(points, true, c, 0.0);
    }

    fn path(&self, points: &[(f32, f32)], fill: bool, c: Rgba, width: f32) {
        if points.len() < 2 {
            return;
        }
        let result: Result<()> = (|| unsafe {
            let path = self.gfx.d2d.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.BeginFigure(
                v2(points[0].0, points[0].1),
                if fill { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW },
            );
            let rest: Vec<Vector2> = points[1..].iter().map(|&(x, y)| v2(x, y)).collect();
            sink.AddLines(&rest);
            sink.EndFigure(if fill { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close()?;
            if fill {
                self.rt.FillGeometry(&path, self.brush(c), None);
            } else {
                self.rt.DrawGeometry(&path, self.brush(c), width, self.round);
            }
            Ok(())
        })();
        let _ = result;
    }

    pub fn text(&self, block: &TextBlock, x: f32, y: f32, c: Rgba) {
        unsafe { self.rt.DrawTextLayout(v2(x, y), &block.layout, self.brush(c), D2D1_DRAW_TEXT_OPTIONS_NONE) };
    }

    /// Lays out and draws `s`; returns the drawn width.
    pub fn text_str(&self, s: &str, format: &IDWriteTextFormat, x: f32, y: f32, c: Rgba) -> f32 {
        match self.gfx.layout(s, format) {
            Ok(b) => {
                self.text(&b, x, y, c);
                b.width
            }
            Err(_) => 0.0,
        }
    }

    pub fn clip(&self, x: f32, y: f32, w: f32, h: f32) {
        unsafe {
            self.rt.PushAxisAlignedClip(
                &D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            )
        };
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }
}
