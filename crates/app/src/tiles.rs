//! Layout and drawing of metric tiles in the taskbar widget.
//!
//! Tiles flow into columns of `rows` tiles each (horizontal taskbar) or a single column (vertical taskbar).
//! Within a column labels, graphs/bars and values are aligned to shared sub-columns, so stacked tiles line up.

use tm_core::color::{Palette, Rgba};
use tm_core::config::{MetricConfig, NetworkUnit, Settings};
use tm_core::history::History;
use tm_core::metrics::{MetricStyle, MetricUnit};
use tm_core::resolver::Reading;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

use crate::render::{Canvas, Gfx, TextBlock};

pub struct TileInput<'a> {
    pub config: &'a MetricConfig,
    pub reading: &'a Reading,
    pub history: Option<&'a History>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

pub struct LaidTile {
    /// Index into the `TileInput` slice.
    pub input: usize,
    pub rect: Rect,
    part_x: f32,
    part_w: f32,
    overlay: bool,
    inset: f32,
}

pub struct WidgetLayout {
    pub width: i32,
    pub height: i32,
    pub tiles: Vec<LaidTile>,
    label_fmt: Option<IDWriteTextFormat>,
    value_fmt: Option<IDWriteTextFormat>,
    small_fmt: Option<IDWriteTextFormat>,
    scale: f32,
}

/// Space available for the widget.
#[derive(Debug, Clone, Copy)]
pub struct Avail {
    pub horizontal: bool,
    /// Taskbar height (horizontal) or width (vertical), physical pixels.
    pub thickness: i32,
    pub scale: f32,
}

fn value_template(input: &TileInput, unit: NetworkUnit) -> &'static str {
    let def = input.config.kind.def();
    match (def.unit, unit) {
        (MetricUnit::BytesPerSecond, NetworkUnit::Bits) if def.source_type == tm_core::metrics::SourceType::Network => {
            "999.9 Mbps"
        }
        _ => def.width_template,
    }
}

pub fn font_px(settings: &Settings, row_h: f32, scale: f32) -> f32 {
    if settings.appearance.font_size > 0.0 {
        settings.appearance.font_size * scale
    } else {
        (row_h * 0.62).clamp(8.0 * scale, 15.0 * scale)
    }
}

pub fn layout(gfx: &Gfx, inputs: &[TileInput], settings: &Settings, avail: Avail) -> WidgetLayout {
    let a = &settings.appearance;
    let s = avail.scale;
    let pad = a.padding * s;
    let margin = settings.layout.vertical_margin * s;
    let thickness = avail.thickness as f32;

    let visible: Vec<usize> = (0..inputs.len())
        .filter(|&i| inputs[i].config.show_in_widget && (inputs[i].reading.available || !inputs[i].config.hide_when_unavailable))
        .collect();

    let rows = if avail.horizontal { settings.layout.rows.max(1) as usize } else { visible.len().max(1) };
    let row_h = if avail.horizontal {
        ((thickness - 2.0 * margin - 2.0 * pad) / rows as f32).max(8.0 * s)
    } else {
        let f = if a.font_size > 0.0 { a.font_size * s } else { 12.0 * s };
        (f * 1.7).round()
    };
    let font = font_px(settings, row_h, s);
    let family = a.font_family.as_str();
    let value_fmt = gfx.text_format(family, font, a.font_bold).ok();
    let label_fmt = gfx.text_format(family, font, false).ok();
    let small_fmt = gfx.text_format(family, (font * 0.72).max(6.0 * s), a.font_bold).ok();
    let gap = (4.0 * s).round();
    let spacing = a.tile_spacing * s;
    let unit = settings.general.network_unit;

    struct Parts {
        label: f32,
        part: f32,
        value: f32,
        overlay: bool,
        fill: bool,
    }
    let parts: Vec<Parts> = visible
        .iter()
        .map(|&i| {
            let t = &inputs[i];
            let c = t.config;
            let label = match (&label_fmt, c.show_label && !t.reading.label.is_empty()) {
                (Some(f), true) => gfx.measure(&t.reading.label, f).0,
                _ => 0.0,
            };
            let value = match (&value_fmt, c.show_value) {
                (Some(f), true) => gfx.measure(value_template(t, unit), f).0.max(gfx.measure(&t.reading.value_text, f).0),
                _ => 0.0,
            };
            let width = |default: f32| (if c.width > 0.0 { c.width } else { default }) * s;
            let part = match c.style {
                MetricStyle::Text | MetricStyle::Fill => 0.0,
                MetricStyle::Bar => width(a.bar_width),
                MetricStyle::VBar => width(5.0).max(3.0),
                MetricStyle::Line | MetricStyle::Area | MetricStyle::Columns => width(a.graph_width),
                MetricStyle::Ring => (row_h - 2.0 * s).max(8.0),
            };
            Parts { label, part, value, overlay: c.overlay_text && part > 0.0, fill: c.style == MetricStyle::Fill }
        })
        .collect();

    let mut tiles = Vec::with_capacity(visible.len());
    let mut x = pad;
    let mut max_bottom: f32 = 0.0;
    for (col, chunk) in parts.chunks(rows).enumerate() {
        let start = col * rows;
        let normal = chunk.iter().filter(|p| !p.overlay);
        let (mut lw, mut pw, mut vw) = (0.0f32, 0.0f32, 0.0f32);
        for p in normal {
            lw = lw.max(p.label);
            pw = pw.max(p.part);
            vw = vw.max(p.value);
        }
        let inset = if chunk.iter().any(|p| p.fill) { (3.0 * s).round() } else { 0.0 };
        let aligned =
            lw + if lw > 0.0 { gap } else { 0.0 } + pw + if pw > 0.0 && vw > 0.0 { gap } else { 0.0 } + vw + 2.0 * inset;
        let overlay_w = chunk
            .iter()
            .filter(|p| p.overlay)
            .map(|p| p.part.max(p.label + if p.label > 0.0 { gap } else { 0.0 } + p.value + 2.0 * gap))
            .fold(0.0f32, f32::max);
        let col_w = aligned.max(overlay_w).ceil();

        for (row, p) in chunk.iter().enumerate() {
            let y = if avail.horizontal {
                ((thickness - 2.0 * margin) - rows as f32 * row_h) / 2.0 + row as f32 * row_h
            } else {
                pad + row as f32 * row_h
            };
            let rect = Rect { x, y, w: col_w, h: row_h };
            let (part_x, part_w) =
                if p.overlay { (x, col_w) } else { (x + inset + lw + if lw > 0.0 { gap } else { 0.0 }, p.part) };
            tiles.push(LaidTile { input: visible[start + row], rect, part_x, part_w, overlay: p.overlay, inset });
            max_bottom = max_bottom.max(y + row_h);
        }
        x += col_w + spacing;
    }
    let content_w = if tiles.is_empty() { 2.0 * pad + 4.0 * s } else { x - spacing + pad };

    let (width, height) = if avail.horizontal {
        (content_w.ceil() as i32, (thickness - 2.0 * margin).round().max(1.0) as i32)
    } else {
        // Vertical taskbar: stretch tiles to the available width.
        let w = (thickness - 2.0 * margin).max(content_w);
        for t in &mut tiles {
            let extra = w - 2.0 * pad - t.rect.w;
            if extra > 0.0 {
                t.rect.w += extra;
                if t.overlay {
                    t.part_w = t.rect.w;
                }
            }
        }
        (w.round() as i32, (max_bottom + pad).ceil() as i32)
    };

    WidgetLayout { width, height, tiles, label_fmt, value_fmt, small_fmt, scale: s }
}

/// Main color of a metric (graphs, bars) before thresholds.
pub fn base_color(config: &MetricConfig, palette: &Palette) -> Rgba {
    Rgba::parse(&config.color).unwrap_or_else(|| palette.category(config.kind.def().category))
}

/// 0..1 position of the current value, scaling rates to the visible history.
pub fn fraction(input: &TileInput) -> f64 {
    if let Some(f) = input.reading.fraction {
        return f;
    }
    let v = input.reading.value;
    if !v.is_finite() {
        return 0.0;
    }
    let max = graph_max(input.config, input.reading, input.history);
    (v / max).clamp(0.0, 1.0)
}

pub fn graph_max(config: &MetricConfig, reading: &Reading, history: Option<&History>) -> f64 {
    if reading.fraction.is_some() && config.kind.def().unit != MetricUnit::Generic {
        return 100.0;
    }
    if config.max_value > 0.0 {
        return config.max_value;
    }
    let hist_max = history.map(History::max).unwrap_or(0.0).max(if reading.value.is_finite() { reading.value } else { 0.0 });
    let floor = if config.kind.def().unit == MetricUnit::BytesPerSecond { 10.0 * 1024.0 } else { 1.0 };
    (hist_max * 1.15).max(floor)
}

pub fn draw(canvas: &Canvas, layout: &WidgetLayout, inputs: &[TileInput], settings: &Settings, palette: &Palette) {
    let a = &settings.appearance;
    let s = layout.scale;
    let gap = (4.0 * s).round();
    for t in &layout.tiles {
        let input = &inputs[t.input];
        let (c, r) = (input.config, input.reading);
        let rect = t.rect;
        let value_color = if !r.available {
            palette.label
        } else if r.uses_thresholds {
            palette.readable(palette.threshold(palette.foreground, r.value, a))
        } else {
            palette.foreground
        };
        let part_color = {
            let base = base_color(c, palette);
            if r.available && r.uses_thresholds { palette.threshold(base, r.value, a) } else { base }
        };
        let frac = fraction(input) as f32;
        // Text over a fill switches to a contrasting color where the fill is (like a progress bar's label).
        let mut text_fill: Option<(Rect, Rgba)> = None;

        // Background part.
        match c.style {
            MetricStyle::Fill => {
                let radius = 3.0 * s;
                canvas.fill_round(rect.x, rect.y + s, rect.w, rect.h - 2.0 * s, radius, palette.track.mul_alpha(0.8));
                if r.available && frac > 0.0 {
                    let (alpha, text) = palette.fill_with_text(part_color);
                    let filled = Rect { x: rect.x, y: rect.y, w: rect.w * frac, h: rect.h };
                    canvas.clip(filled.x, filled.y, filled.w, filled.h);
                    canvas.fill_round(rect.x, rect.y + s, rect.w, rect.h - 2.0 * s, radius, part_color.with_alpha(alpha));
                    canvas.unclip();
                    text_fill = Some((filled, text));
                }
            }
            MetricStyle::Bar if t.part_w > 0.0 => {
                let bh = (rect.h * 0.3).clamp(3.0 * s, 6.0 * s).round();
                let by = rect.y + (rect.h - bh) / 2.0;
                let bh_ov = if t.overlay { rect.h - 2.0 * s } else { bh };
                let by_ov = if t.overlay { rect.y + s } else { by };
                canvas.fill_round(t.part_x, by_ov, t.part_w, bh_ov, bh / 2.0, palette.track);
                if r.available {
                    let (fill_alpha, text) =
                        if t.overlay { palette.fill_with_text(part_color) } else { (1.0, palette.foreground) };
                    // Keep a visible rounded cap for small non-zero values.
                    let fw = t.part_w * frac;
                    let fw = if fw > 0.0 { fw.max(bh.min(t.part_w)) } else { 0.0 };
                    canvas.fill_round(t.part_x, by_ov, fw, bh_ov, bh / 2.0, part_color.mul_alpha(fill_alpha));
                    if t.overlay && fw > 0.0 {
                        text_fill = Some((Rect { x: t.part_x, y: rect.y, w: fw, h: rect.h }, text));
                    }
                }
            }
            MetricStyle::VBar if t.part_w > 0.0 => {
                let h = rect.h - 4.0 * s;
                let y = rect.y + 2.0 * s;
                canvas.fill_round(t.part_x, y, t.part_w, h, t.part_w / 3.0, palette.track);
                if r.available {
                    let fh = h * frac;
                    canvas.fill_round(t.part_x, y + h - fh, t.part_w, fh, t.part_w / 3.0, part_color);
                }
            }
            MetricStyle::Line | MetricStyle::Area | MetricStyle::Columns if t.part_w > 0.0 => {
                let gy = rect.y + 1.5 * s;
                let gh = rect.h - 3.0 * s;
                let max = graph_max(c, r, input.history);
                canvas.fill_round(t.part_x, gy, t.part_w, gh, 2.0 * s, palette.track.mul_alpha(0.55));
                if let Some(h) = input.history {
                    draw_graph(canvas, h, c.style, t.part_x, gy, t.part_w, gh, max, part_color, s);
                }
            }
            MetricStyle::Ring if t.part_w > 0.0 => {
                let d = if t.overlay { rect.h - 2.0 * s } else { t.part_w };
                let cx = if t.overlay { rect.x + rect.w / 2.0 } else { t.part_x + d / 2.0 };
                let cy = rect.y + rect.h / 2.0;
                let stroke = (d * 0.15).max(2.0 * s);
                let radius = d / 2.0 - stroke / 2.0;
                canvas.stroke_circle(cx, cy, radius, palette.track, stroke);
                if r.available {
                    canvas.arc(cx, cy, radius, frac, part_color, stroke);
                }
                if t.overlay {
                    if let Some(f) = &layout.small_fmt {
                        let text = r.value_text.trim_end_matches('%');
                        if let Ok(b) = canvas.gfx.layout(text, f) {
                            canvas.text(&b, cx - b.width / 2.0, cy - b.height / 2.0, value_color);
                        }
                    }
                    continue;
                }
            }
            _ => {}
        }

        // Text.
        let draw_text = |b: &TextBlock, x: f32, normal: Rgba, on_fill_alpha: f32| {
            let y = rect.y + (rect.h - b.height) / 2.0;
            match text_fill {
                Some((fill, on_fill)) if x < fill.x + fill.w => {
                    let split = fill.x + fill.w;
                    canvas.clip(rect.x, rect.y, split - rect.x, rect.h);
                    canvas.text(b, x, y, on_fill.mul_alpha(on_fill_alpha));
                    canvas.unclip();
                    if x + b.width > split {
                        canvas.clip(split, rect.y, rect.x + rect.w - split, rect.h);
                        canvas.text(b, x, y, normal);
                        canvas.unclip();
                    }
                }
                _ => canvas.text(b, x, y, normal),
            }
        };
        if c.show_label
            && !r.label.is_empty()
            && let Some(f) = &layout.label_fmt
            && let Ok(b) = canvas.gfx.layout(&r.label, f)
        {
            let lx = if t.overlay { rect.x + gap } else { rect.x + t.inset };
            draw_text(&b, lx, palette.label, 0.85);
        }
        if c.show_value
            && let Some(f) = &layout.value_fmt
            && let Ok(b) = canvas.gfx.layout(&r.value_text, f)
        {
            let right = rect.x + rect.w - if t.overlay { gap } else { t.inset };
            draw_text(&b, right - b.width, value_color, 1.0);
        }
    }
}

/// History graph scaled to `max`, newest sample at the right edge.
#[allow(clippy::too_many_arguments)]
pub fn draw_graph(
    canvas: &Canvas,
    h: &History,
    style: MetricStyle,
    x: f32,
    y: f32,
    w: f32,
    gh: f32,
    max: f64,
    color: Rgba,
    s: f32,
) {
    let n = h.capacity().max(2);
    let step = w / (n - 1) as f32;
    let len = h.len();
    let to_y = |v: f64| y + gh - (gh * (v / max).clamp(0.0, 1.0) as f32);
    canvas.clip(x, y, w, gh);
    match style {
        MetricStyle::Columns => {
            let bw = (step * 0.75).max(1.0);
            for (i, v) in h.iter().enumerate() {
                if !v.is_finite() {
                    continue;
                }
                let px = x + w - (len - 1 - i) as f32 * step - bw / 2.0;
                let top = to_y(v);
                canvas.fill_rect(px, top, bw, y + gh - top, color.mul_alpha(0.85));
            }
        }
        _ => {
            // Split at missing samples so gaps stay visible.
            let mut segment: Vec<(f32, f32)> = Vec::with_capacity(len);
            let flush = |segment: &mut Vec<(f32, f32)>| {
                if segment.len() >= 2 {
                    if style == MetricStyle::Area || style == MetricStyle::Line {
                        let mut poly = segment.clone();
                        poly.push((segment[segment.len() - 1].0, y + gh));
                        poly.push((segment[0].0, y + gh));
                        let alpha = if style == MetricStyle::Area { 0.45 } else { 0.18 };
                        canvas.fill_polygon(&poly, color.mul_alpha(alpha));
                    }
                    canvas.polyline(segment, color, (1.3 * s).max(1.0));
                }
                segment.clear();
            };
            for (i, v) in h.iter().enumerate() {
                if v.is_finite() {
                    segment.push((x + w - (len - 1 - i) as f32 * step, to_y(v)));
                } else {
                    flush(&mut segment);
                }
            }
            flush(&mut segment);
        }
    }
    canvas.unclip();
}
