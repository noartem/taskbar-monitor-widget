//! Settings window (separate process, started on demand so the widget itself stays tiny).
//!
//! Every change is written to `config.toml` after a short debounce; the running widget notices the new
//! modification time and applies it live.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, ComboBox, DragValue, RichText, ScrollArea, Slider, TextEdit};
use tm_core::claude::{ClaudeStatus, ClaudeUsage, resolve_credentials_path};
use tm_core::color::Rgba;
use tm_core::config::{
    self, AppPaths, Autostart, ClickAction, EmbedMode, GpuDisplay, MetricConfig, MonitorMode, NetworkUnit, Placement, Settings,
    ThemeMode, TrayIconMode,
};
use tm_core::i18n::{self, tr, trf};
use tm_core::metrics::{MetricKind, MetricStyle, SourceType};
use tm_core::presets;

use crate::app::{MAIN_CLASS, SETTINGS_TITLE, WM_APP_RESTART_ELEVATED};
use crate::{autostart, collector, elevation, theme, util};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Layout,
    Metrics,
    Claude,
    Hardware,
    About,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::General,
        Section::Appearance,
        Section::Layout,
        Section::Metrics,
        Section::Claude,
        Section::Hardware,
        Section::About,
    ];

    fn key(self) -> &'static str {
        match self {
            Section::General => "set.section.general",
            Section::Appearance => "set.section.appearance",
            Section::Layout => "set.section.layout",
            Section::Metrics => "set.section.metrics",
            Section::Claude => "set.section.claude",
            Section::Hardware => "set.section.hardware",
            Section::About => "set.section.about",
        }
    }
}

/// Devices and sensors offered as metric sources.
#[derive(Default, Clone)]
struct Sources {
    gpus: Vec<String>,
    gpu_diag: GpuDiag,
    networks: Vec<(String, String)>,
    disks: Vec<String>,
    drives: Vec<String>,
    sensors: Vec<(String, String)>,
}

/// What the widget's GPU pipeline sees: DXGI adapters, raw counter instances and the merged result.
#[derive(Default, Clone)]
struct GpuDiag {
    rows: Vec<tm_core::snapshot::GpuInfo>,
    text: String,
}

fn gpu_diagnostics() -> GpuDiag {
    let adapters = collector::gpu::adapters();
    let mut text = String::from("DXGI adapters:\n");
    for a in &adapters {
        text += &format!("  {} [{}] {} MB\n", a.name, a.luid_str, a.dedicated >> 20);
    }
    let Some(pdh) = collector::pdh::Pdh::open(collector::pdh::PdhNeeds { gpu: true, ..Default::default() }) else {
        text += "PDH query could not be opened\n";
        return GpuDiag { rows: Vec::new(), text };
    };
    // Utilization is a rate counter: it needs two samples.
    pdh.collect();
    std::thread::sleep(Duration::from_millis(1000));
    pdh.collect();
    let (engine, memory) = pdh.gpu_raw();
    match &engine {
        Some(e) => {
            text += &format!("GPU Engine instances: {}\n", e.len());
            for (name, v) in e.iter().filter(|(_, v)| *v > 0.0).take(5) {
                text += &format!("  {name} = {v:.1}\n");
            }
        }
        None => text += "GPU Engine counter: unavailable\n",
    }
    match &memory {
        Some(m) => {
            text += &format!("GPU Adapter Memory instances: {}\n", m.len());
            for (name, v) in m.iter().take(8) {
                text += &format!("  {name} = {} MB\n", *v as u64 >> 20);
            }
        }
        None => text += "GPU Adapter Memory counter: unavailable\n",
    }
    let usage = pdh.gpu_usage();
    let dedicated = pdh.gpu_dedicated();
    let rows = collector::merge_gpus(&adapters, usage.as_ref(), dedicated.as_ref(), &std::collections::HashMap::new());
    text += "Result:\n";
    for g in &rows {
        text += &format!(
            "  {} [{}] load={:?} vram={} / {} MB default={} listed={}\n",
            g.name,
            g.luid,
            g.usage.map(|u| (u * 10.0).round() / 10.0),
            g.vram_used >> 20,
            g.vram_total >> 20,
            g.is_default,
            g.listed
        );
    }
    GpuDiag { rows, text }
}

fn enumerate_sources() -> Sources {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    collector::wmi::init_security();
    let mut s = Sources {
        gpus: collector::gpu::adapters().into_iter().map(|a| a.name).collect(),
        networks: collector::network::interfaces().into_iter().map(|i| (i.name, i.description)).collect(),
        drives: collector::system::drives().into_iter().map(|d| d.name).collect(),
        gpu_diag: gpu_diagnostics(),
        ..Default::default()
    };
    if let Some(pdh) = collector::pdh::Pdh::open(collector::pdh::PdhNeeds { disk: true, ..Default::default() }) {
        pdh.collect();
        std::thread::sleep(Duration::from_millis(300));
        pdh.collect();
        s.disks = pdh.disks().into_iter().map(|d| d.name).collect();
    }
    if let Some(mut wmi) = collector::wmi::Wmi::new() {
        s.sensors = wmi
            .lhm_sensors()
            .into_iter()
            .map(|x| {
                let label = format!("{} · {} ({})", x.hardware, x.name, x.unit);
                (x.id, label)
            })
            .collect();
        s.sensors.sort_by(|a, b| a.1.cmp(&b.1));
    }
    s
}

struct SettingsApp {
    paths: AppPaths,
    settings: Settings,
    saved: Settings,
    dirty_since: Option<Instant>,
    file_mtime: Option<SystemTime>,
    last_file_check: Instant,
    section: Section,
    selected: usize,
    add_kind: MetricKind,
    sources: Arc<Mutex<Option<Sources>>>,
    autostart_actual: Autostart,
    autostart_job: Option<Receiver<Result<bool, String>>>,
    claude_test: Option<Receiver<ClaudeUsage>>,
    claude_result: Option<ClaudeUsage>,
    elevated: bool,
    status: Option<(String, Instant)>,
    confirm_reset: bool,
    applied_theme: Option<ThemeMode>,
    query: String,
    focus: Focus,
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn rgba_to_srgb(hex: &str, fallback: [u8; 3]) -> [u8; 3] {
    Rgba::parse(hex)
        .map(|c| [(c.r * 255.0).round() as u8, (c.g * 255.0).round() as u8, (c.b * 255.0).round() as u8])
        .unwrap_or(fallback)
}

fn srgb_to_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Color picker bound to a `#RRGGBB` string.
fn color_field(ui: &mut egui::Ui, value: &mut String) {
    let mut rgb = rgba_to_srgb(value, [255, 255, 255]);
    if ui.color_edit_button_srgb(&mut rgb).changed() {
        *value = srgb_to_hex(rgb);
    }
    ui.add(TextEdit::singleline(value).desired_width(80.0));
}

fn kind_name(k: MetricKind) -> &'static str {
    tr(&format!("kind.{}", k.key()))
}

fn style_name(s: MetricStyle) -> &'static str {
    tr(&format!("style.{}", s.key()))
}

impl SettingsApp {
    fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths) -> Self {
        let (settings, _) = config::load(&paths.config_file());
        i18n::init(&settings.general.language, "");
        setup_fonts(&cc.egui_ctx);
        let sources = Arc::new(Mutex::new(None));
        {
            let sources = sources.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let s = enumerate_sources();
                *sources.lock().unwrap() = Some(s);
                ctx.request_repaint();
            });
        }
        Self {
            file_mtime: mtime(&paths.config_file()),
            paths,
            saved: settings.clone(),
            settings,
            dirty_since: None,
            last_file_check: Instant::now(),
            section: Section::General,
            selected: 0,
            add_kind: MetricKind::CpuUsage,
            sources,
            autostart_actual: autostart::current(),
            autostart_job: None,
            claude_test: None,
            claude_result: None,
            elevated: elevation::is_elevated(),
            status: None,
            confirm_reset: false,
            applied_theme: None,
            query: String::new(),
            focus: Focus::default(),
        }
    }

    fn flash(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    fn save_now(&mut self) {
        self.settings.normalize();
        match config::save(&self.paths.config_file(), &self.settings) {
            Ok(()) => {
                self.saved = self.settings.clone();
                self.file_mtime = mtime(&self.paths.config_file());
                self.flash(tr("set.saved"));
            }
            Err(e) => self.flash(format!("{}: {e}", tr("dlg.error"))),
        }
    }

    /// Debounced autosave plus pick-up of external edits (e.g. the widget saving a dragged position).
    fn sync_file(&mut self, ctx: &egui::Context) {
        if self.settings != self.saved {
            let since = *self.dirty_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(350) {
                self.dirty_since = None;
                if self.settings.general.language != self.saved.general.language {
                    i18n::init(&self.settings.general.language, "");
                }
                self.save_now();
            } else {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
        } else if self.last_file_check.elapsed() >= Duration::from_secs(1) {
            self.last_file_check = Instant::now();
            let m = mtime(&self.paths.config_file());
            if m.is_some() && m != self.file_mtime {
                self.file_mtime = m;
                if let Ok(Ok(s)) = std::fs::read_to_string(self.paths.config_file()).map(|t| Settings::from_toml(&t)) {
                    self.saved = s.clone();
                    self.settings = s;
                }
            }
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let mode = self.settings.appearance.theme;
        if self.applied_theme == Some(mode) {
            return;
        }
        self.applied_theme = Some(mode);
        let pref = match mode {
            ThemeMode::Light => egui::ThemePreference::Light,
            ThemeMode::Dark => egui::ThemePreference::Dark,
            ThemeMode::System => {
                if theme::apps_are_dark() {
                    egui::ThemePreference::Dark
                } else {
                    egui::ThemePreference::Light
                }
            }
            ThemeMode::Custom => {
                if theme::taskbar_is_dark() {
                    egui::ThemePreference::Dark
                } else {
                    egui::ThemePreference::Light
                }
            }
        };
        ctx.set_theme(pref);
        let accent =
            theme::accent_color().map(|c| Color32::from_rgb((c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8));
        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 7.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
            style.spacing.interact_size.y = 24.0;
            if let Some(a) = accent {
                style.visuals.selection.bg_fill = a;
                style.visuals.hyperlink_color = a;
            }
            for (text_style, size) in [
                (egui::TextStyle::Body, 14.0),
                (egui::TextStyle::Button, 14.0),
                (egui::TextStyle::Heading, 22.0),
                (egui::TextStyle::Small, 11.5),
                (egui::TextStyle::Monospace, 13.0),
            ] {
                if let Some(f) = style.text_styles.get_mut(&text_style) {
                    f.size = size;
                }
            }
        });
    }

    fn poll_jobs(&mut self) {
        if let Some(rx) = &self.autostart_job
            && let Ok(result) = rx.try_recv()
        {
            self.autostart_job = None;
            self.autostart_actual = autostart::current();
            match result {
                Ok(true) => {
                    self.settings.general.autostart = self.autostart_actual;
                    self.flash(tr("set.saved"));
                }
                Ok(false) => self.flash(tr("set.autostart.cancelled")),
                Err(e) => self.flash(format!("{}: {e}", tr("dlg.error"))),
            }
        }
        if let Some(rx) = &self.claude_test
            && let Ok(result) = rx.try_recv()
        {
            self.claude_test = None;
            self.claude_result = Some(result);
        }
    }

    /// Opens the section of a search hit and highlights the setting.
    fn jump_to(&mut self, section: Section, key: &'static str) {
        self.section = section;
        self.query.clear();
        if let Some(kind) = MetricKind::ALL.into_iter().find(|k| tr_key_static(*k) == key) {
            if let Some(i) = self.settings.metrics.iter().position(|m| m.kind == kind) {
                self.selected = i;
            } else {
                self.add_kind = kind;
            }
            self.focus = Focus::new("set.metric.kind");
        } else {
            self.focus = Focus::new(key);
        }
    }

    fn search_results(&mut self, ui: &mut egui::Ui) {
        let hits = search(&self.query);
        if hits.is_empty() {
            ui.label(RichText::new(tr("set.search.none")).weak());
            return;
        }
        let mut chosen = None;
        for (section, key) in &hits {
            let text = format!("{}  ›  {}", tr(section.key()), tr(key));
            if ui.add(egui::Button::new(text).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                chosen = Some((*section, *key));
            }
        }
        if let Some((section, key)) = chosen {
            self.jump_to(section, key);
        }
    }

    // ---- sections -----------------------------------------------------------------------------------------

    fn general(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        let g = &mut self.settings.general;
        egui::Grid::new("general").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            mark(&focus, "set.language", &ui.label(tr("set.language")));
            ComboBox::from_id_salt("lang")
                .selected_text(match g.language.as_str() {
                    "en" => "English",
                    "ru" => "Русский",
                    _ => tr("set.language.auto"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut g.language, "auto".into(), tr("set.language.auto"));
                    ui.selectable_value(&mut g.language, "en".into(), "English");
                    ui.selectable_value(&mut g.language, "ru".into(), "Русский");
                });
            ui.end_row();

            mark(&focus, "set.refresh", &ui.label(tr("set.refresh")));
            ui.add(Slider::new(&mut g.refresh_interval_ms, 250..=5000).suffix(" ms").step_by(250.0));
            ui.end_row();

            mark(&focus, "set.history", &ui.label(tr("set.history")));
            ui.add(Slider::new(&mut g.history_length, 10..=600).suffix(format!(" {}", tr("set.samples"))));
            ui.end_row();

            mark(&focus, "set.network_unit", &ui.label(tr("set.network_unit")));
            ui.horizontal(|ui| {
                ui.radio_value(&mut g.network_unit, NetworkUnit::Bytes, "MB/s");
                ui.radio_value(&mut g.network_unit, NetworkUnit::Bits, "Mbps");
            });
            ui.end_row();

            for (label, value, id) in [
                (tr("set.left_click"), &mut g.left_click, "lc"),
                (tr("set.double_click"), &mut g.double_click, "dc"),
                (tr("set.middle_click"), &mut g.middle_click, "mc"),
            ] {
                ui.label(label);
                ComboBox::from_id_salt(id).selected_text(tr(value.key())).show_ui(ui, |ui| {
                    for a in ClickAction::ALL {
                        ui.selectable_value(value, a, tr(a.key()));
                    }
                });
                ui.end_row();
            }
        });

        ui.add_space(12.0);
        mark(&focus, "menu.autostart", &ui.heading(tr("menu.autostart")));
        ui.label(RichText::new(tr("set.autostart.hint")).weak());
        let busy = self.autostart_job.is_some();
        let mut chosen = self.autostart_actual;
        ui.add_enabled_ui(!busy, |ui| {
            ui.radio_value(&mut chosen, Autostart::Off, tr("autostart.off"));
            ui.radio_value(&mut chosen, Autostart::User, tr("autostart.user"));
            ui.radio_value(&mut chosen, Autostart::Elevated, tr("autostart.elevated"));
        });
        if busy {
            ui.spinner();
        }
        if chosen != self.autostart_actual && !busy {
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(autostart::set_with_elevation(chosen).map_err(|e| e.to_string()));
            });
            self.autostart_job = Some(rx);
        }
    }

    fn appearance(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        let a = &mut self.settings.appearance;
        egui::Grid::new("appearance").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            mark(&focus, "set.theme", &ui.label(tr("set.theme")));
            ComboBox::from_id_salt("theme").selected_text(tr(theme_key(a.theme))).show_ui(ui, |ui| {
                for t in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark, ThemeMode::Custom] {
                    ui.selectable_value(&mut a.theme, t, tr(theme_key(t)));
                }
            });
            ui.end_row();

            if a.theme == ThemeMode::Custom {
                mark(&focus, "set.text_color", &ui.label(tr("set.text_color")));
                ui.horizontal(|ui| color_field(ui, &mut a.foreground_color));
                ui.end_row();
            }
            mark(&focus, "set.background", &ui.label(tr("set.background")));
            ui.horizontal(|ui| {
                color_field(ui, &mut a.background_color);
                ui.add(Slider::new(&mut a.background_opacity, 0.0..=1.0).text(tr("set.opacity")));
            });
            ui.end_row();

            mark(&focus, "set.corner_radius", &ui.label(tr("set.corner_radius")));
            ui.add(Slider::new(&mut a.corner_radius, 0.0..=16.0));
            ui.end_row();

            mark(&focus, "set.font", &ui.label(tr("set.font")));
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut a.font_family).desired_width(200.0));
                mark(&focus, "set.bold", &ui.checkbox(&mut a.font_bold, tr("set.bold")));
            });
            ui.end_row();

            mark(&focus, "set.font_size", &ui.label(tr("set.font_size")));
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut a.font_size).range(0.0..=40.0).speed(0.25));
                ui.label(RichText::new(tr("set.zero_auto")).weak());
            });
            ui.end_row();

            mark(&focus, "set.label_opacity", &ui.label(tr("set.label_opacity")));
            ui.add(Slider::new(&mut a.label_opacity, 0.1..=1.0));
            ui.end_row();

            mark(&focus, "set.accent", &ui.label(tr("set.accent")));
            ui.horizontal(|ui| {
                mark(&focus, "set.system_accent", &ui.checkbox(&mut a.use_system_accent, tr("set.system_accent")));
                if !a.use_system_accent {
                    color_field(ui, &mut a.accent_color);
                }
            });
            ui.end_row();

            ui.label("");
            mark(&focus, "set.color_by_category", &ui.checkbox(&mut a.color_by_category, tr("set.color_by_category")));
            ui.end_row();

            mark(&focus, "set.thresholds", &ui.label(tr("set.thresholds")));
            mark(&focus, "set.use_thresholds", &ui.checkbox(&mut a.use_threshold_colors, tr("set.use_thresholds")));
            ui.end_row();
            if a.use_threshold_colors {
                mark(&focus, "set.warn", &ui.label(tr("set.warn")));
                ui.horizontal(|ui| {
                    ui.add(Slider::new(&mut a.warn_threshold, 0.0..=100.0).suffix("%"));
                    color_field(ui, &mut a.warn_color);
                });
                ui.end_row();
                mark(&focus, "set.crit", &ui.label(tr("set.crit")));
                ui.horizontal(|ui| {
                    ui.add(Slider::new(&mut a.crit_threshold, 0.0..=100.0).suffix("%"));
                    color_field(ui, &mut a.crit_color);
                });
                ui.end_row();
            }

            mark(&focus, "set.padding", &ui.label(tr("set.padding")));
            ui.add(Slider::new(&mut a.padding, 0.0..=16.0));
            ui.end_row();
            mark(&focus, "set.tile_spacing", &ui.label(tr("set.tile_spacing")));
            ui.add(Slider::new(&mut a.tile_spacing, 0.0..=40.0));
            ui.end_row();
            mark(&focus, "set.graph_width", &ui.label(tr("set.graph_width")));
            ui.add(Slider::new(&mut a.graph_width, 8.0..=160.0));
            ui.end_row();
            mark(&focus, "set.bar_width", &ui.label(tr("set.bar_width")));
            ui.add(Slider::new(&mut a.bar_width, 4.0..=160.0));
            ui.end_row();
        });
    }

    fn layout(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        mark(&focus, "menu.layout", &ui.heading(tr("menu.layout")));
        ui.label(RichText::new(tr("set.presets.hint")).weak());
        ui.horizontal_wrapped(|ui| {
            for p in presets::ALL {
                if ui.button(tr(&format!("preset.{}", p.key))).clicked() {
                    presets::apply(&mut self.settings, p);
                    self.selected = 0;
                }
            }
        });
        ui.add_space(10.0);

        let l = &mut self.settings.layout;
        egui::Grid::new("layout").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            mark(&focus, "set.placement", &ui.label(tr("set.placement")));
            ComboBox::from_id_salt("placement").selected_text(tr(placement_key(l.placement))).show_ui(ui, |ui| {
                for p in [Placement::NearTray, Placement::Left, Placement::Center] {
                    ui.selectable_value(&mut l.placement, p, tr(placement_key(p)));
                }
            });
            ui.end_row();

            mark(&focus, "set.offset", &ui.label(tr("set.offset")));
            ui.horizontal(|ui| {
                ui.label("X");
                ui.add(DragValue::new(&mut l.offset_x).speed(1.0));
                ui.label("Y");
                ui.add(DragValue::new(&mut l.offset_y).speed(0.5).range(-100.0..=100.0));
            });
            ui.end_row();

            mark(&focus, "menu.rows", &ui.label(tr("menu.rows")));
            ui.add(Slider::new(&mut l.rows, 1..=4));
            ui.end_row();

            mark(&focus, "set.vertical_margin", &ui.label(tr("set.vertical_margin")));
            ui.add(Slider::new(&mut l.vertical_margin, 0.0..=12.0));
            ui.end_row();

            mark(&focus, "set.monitor", &ui.label(tr("set.monitor")));
            ui.horizontal(|ui| {
                ComboBox::from_id_salt("monitor").selected_text(tr(monitor_key(l.monitor))).show_ui(ui, |ui| {
                    for m in [MonitorMode::Primary, MonitorMode::All, MonitorMode::Specific] {
                        ui.selectable_value(&mut l.monitor, m, tr(monitor_key(m)));
                    }
                });
                if l.monitor == MonitorMode::Specific {
                    ui.add(DragValue::new(&mut l.monitor_index).range(0..=8));
                }
            });
            ui.end_row();

            mark(&focus, "set.secondary_margin", &ui.label(tr("set.secondary_margin")));
            ui.add(Slider::new(&mut l.secondary_right_margin, 0.0..=400.0));
            ui.end_row();

            mark(&focus, "set.mode", &ui.label(tr("set.mode")));
            ComboBox::from_id_salt("embed")
                .selected_text(if l.embed == EmbedMode::Overlay { tr("set.mode.overlay") } else { tr("set.mode.embedded") })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut l.embed, EmbedMode::Overlay, tr("set.mode.overlay"));
                    ui.selectable_value(&mut l.embed, EmbedMode::Embedded, tr("set.mode.embedded"));
                });
            ui.end_row();

            ui.label("");
            ui.vertical(|ui| {
                mark(&focus, "menu.lock", &ui.checkbox(&mut l.lock_position, tr("menu.lock")));
                if !l.lock_position {
                    ui.label(RichText::new(tr("set.drag_hint")).weak());
                }
                mark(&focus, "set.hide_fullscreen", &ui.checkbox(&mut l.hide_on_fullscreen, tr("set.hide_fullscreen")));
            });
            ui.end_row();
        });

        ui.add_space(12.0);
        mark(&focus, "set.tray", &ui.heading(tr("set.tray")));
        let t = &mut self.settings.tray;
        mark(&focus, "set.tray.show", &ui.checkbox(&mut t.show_icon, tr("set.tray.show")));
        ui.add_enabled_ui(t.show_icon, |ui| {
            mark(&focus, "set.tray.tooltip", &ui.checkbox(&mut t.show_tooltip, tr("set.tray.tooltip")));
            ui.horizontal(|ui| {
                mark(
                    &focus,
                    "set.tray.app_icon",
                    &ui.radio_value(&mut t.icon_mode, TrayIconMode::AppIcon, tr("set.tray.app_icon")),
                );
                mark(
                    &focus,
                    "set.tray.metric_icon",
                    &ui.radio_value(&mut t.icon_mode, TrayIconMode::Metric, tr("set.tray.metric_icon")),
                );
                if t.icon_mode == TrayIconMode::Metric {
                    ComboBox::from_id_salt("tray_metric").selected_text(kind_name(t.icon_metric)).show_ui(ui, |ui| {
                        for k in MetricKind::ALL.into_iter().filter(|k| {
                            k.def().unit == tm_core::metrics::MetricUnit::Percent
                                || *k == MetricKind::CpuTemp
                                || *k == MetricKind::GpuTemp
                        }) {
                            ui.selectable_value(&mut t.icon_metric, k, kind_name(k));
                        }
                    });
                }
            });
        });
    }

    fn metrics(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        ui.label(RichText::new(tr("set.metrics.hint")).weak());
        ui.add_space(6.0);
        let sources = self.sources.lock().unwrap().clone();
        let count = self.settings.metrics.len();
        if self.selected >= count {
            self.selected = count.saturating_sub(1);
        }

        ui.columns(2, |cols| {
            // List.
            let ui = &mut cols[0];
            ui.horizontal(|ui| {
                ComboBox::from_id_salt("add_kind").width(200.0).selected_text(kind_name(self.add_kind)).show_ui(ui, |ui| {
                    for k in MetricKind::ALL {
                        ui.selectable_value(&mut self.add_kind, k, kind_name(k));
                    }
                });
                if ui.button(format!("+ {}", tr("set.add"))).clicked() {
                    self.settings.metrics.push(MetricConfig::new(self.add_kind));
                    self.selected = self.settings.metrics.len() - 1;
                }
            });
            ui.label(RichText::new(tr("set.metrics.columns")).small().weak());
            ScrollArea::vertical().id_salt("metric_list").max_height(ui.available_height() - 8.0).show(ui, |ui| {
                let mut action: Option<(usize, i32)> = None;
                for i in 0..self.settings.metrics.len() {
                    let m = &mut self.settings.metrics[i];
                    ui.horizontal(|ui| {
                        if ui.small_button("▲").clicked() {
                            action = Some((i, -1));
                        }
                        if ui.small_button("▼").clicked() {
                            action = Some((i, 1));
                        }
                        mark(
                            &focus,
                            "set.show_widget",
                            &ui.checkbox(&mut m.show_in_widget, "").on_hover_text(tr("set.show_widget")),
                        );
                        mark(
                            &focus,
                            "set.show_flyout",
                            &ui.checkbox(&mut m.show_in_flyout, "").on_hover_text(tr("set.show_flyout")),
                        );
                        let name = if m.label.is_empty() {
                            kind_name(m.kind).to_string()
                        } else {
                            format!("{} ({})", m.label, kind_name(m.kind))
                        };
                        let name = if m.source.is_empty() { name } else { format!("{name} · {}", m.source) };
                        if ui.selectable_label(self.selected == i, name).clicked() {
                            self.selected = i;
                        }
                    });
                }
                if let Some((i, dir)) = action {
                    let j = i as i32 + dir;
                    if j >= 0 && (j as usize) < self.settings.metrics.len() {
                        self.settings.metrics.swap(i, j as usize);
                        self.selected = j as usize;
                    }
                }
            });

            // Details.
            let ui = &mut cols[1];
            let Some(m) = self.settings.metrics.get_mut(self.selected) else {
                mark(&focus, "set.metrics.empty", &ui.label(tr("set.metrics.empty")));
                return;
            };
            let mut remove = false;
            let mut duplicate = false;
            egui::Grid::new("metric_detail").num_columns(2).spacing([16.0, 9.0]).show(ui, |ui| {
                mark(&focus, "set.metric.kind", &ui.label(tr("set.metric.kind")));
                let before = m.kind;
                ComboBox::from_id_salt("kind").width(220.0).selected_text(kind_name(m.kind)).show_ui(ui, |ui| {
                    for k in MetricKind::ALL {
                        ui.selectable_value(&mut m.kind, k, kind_name(k));
                    }
                });
                if m.kind != before {
                    m.source.clear();
                    m.style = m.kind.def().default_style;
                }
                ui.end_row();

                let source_type = m.kind.def().source_type;
                if source_type != SourceType::None {
                    mark(&focus, "set.metric.source", &ui.label(tr("set.metric.source")));
                    ui.vertical(|ui| {
                        source_picker(ui, m, source_type, sources.as_ref());
                    });
                    ui.end_row();
                }

                mark(&focus, "set.metric.label", &ui.label(tr("set.metric.label")));
                ui.add(TextEdit::singleline(&mut m.label).hint_text(m.kind.def().default_label).desired_width(160.0));
                ui.end_row();

                mark(&focus, "set.metric.style", &ui.label(tr("set.metric.style")));
                ComboBox::from_id_salt("style").selected_text(style_name(m.style)).show_ui(ui, |ui| {
                    for s in MetricStyle::ALL {
                        ui.selectable_value(&mut m.style, s, style_name(s));
                    }
                });
                ui.end_row();

                mark(&focus, "set.metric.show", &ui.label(tr("set.metric.show")));
                ui.vertical(|ui| {
                    mark(&focus, "set.show_widget", &ui.checkbox(&mut m.show_in_widget, tr("set.show_widget")));
                    mark(&focus, "set.show_flyout", &ui.checkbox(&mut m.show_in_flyout, tr("set.show_flyout")));
                    ui.horizontal(|ui| {
                        mark(&focus, "set.metric.show_label", &ui.checkbox(&mut m.show_label, tr("set.metric.show_label")));
                        mark(&focus, "set.metric.show_value", &ui.checkbox(&mut m.show_value, tr("set.metric.show_value")));
                    });
                    if matches!(
                        m.style,
                        MetricStyle::Line | MetricStyle::Area | MetricStyle::Columns | MetricStyle::Ring | MetricStyle::Bar
                    ) {
                        mark(&focus, "set.metric.overlay", &ui.checkbox(&mut m.overlay_text, tr("set.metric.overlay")));
                    }
                    mark(
                        &focus,
                        "set.metric.hide_unavailable",
                        &ui.checkbox(&mut m.hide_when_unavailable, tr("set.metric.hide_unavailable")),
                    );
                });
                ui.end_row();

                mark(&focus, "set.metric.color", &ui.label(tr("set.metric.color")));
                ui.horizontal(|ui| {
                    let mut custom = !m.color.is_empty();
                    if ui.checkbox(&mut custom, tr("set.custom")).changed() {
                        m.color = if custom { "#4CC2FF".into() } else { String::new() };
                    }
                    if custom {
                        color_field(ui, &mut m.color);
                    }
                });
                ui.end_row();

                mark(&focus, "set.metric.width", &ui.label(tr("set.metric.width")));
                ui.horizontal(|ui| {
                    ui.add(DragValue::new(&mut m.width).range(0.0..=400.0));
                    ui.label(RichText::new(tr("set.zero_auto")).weak());
                });
                ui.end_row();

                mark(&focus, "set.metric.decimals", &ui.label(tr("set.metric.decimals")));
                ui.add(Slider::new(&mut m.decimals, -1..=3).text(tr("set.minus_one_auto")));
                ui.end_row();

                if m.kind.def().unit != tm_core::metrics::MetricUnit::Percent {
                    mark(&focus, "set.metric.max", &ui.label(tr("set.metric.max")));
                    ui.horizontal(|ui| {
                        ui.add(DragValue::new(&mut m.max_value).range(0.0..=1e12).speed(1024.0));
                        ui.label(RichText::new(tr("set.metric.max_hint")).weak());
                    });
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("set.duplicate")).clicked() {
                    duplicate = true;
                }
                if ui.button(RichText::new(tr("set.remove")).color(Color32::from_rgb(0xE8, 0x5A, 0x5A))).clicked() {
                    remove = true;
                }
            });
            if duplicate {
                let copy = self.settings.metrics[self.selected].clone();
                self.settings.metrics.insert(self.selected + 1, copy);
                self.selected += 1;
            } else if remove {
                self.settings.metrics.remove(self.selected);
            }
        });
    }

    fn claude(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        ui.label(RichText::new(tr("set.claude.hint")).weak());
        ui.add_space(6.0);
        let c = &mut self.settings.claude;
        mark(&focus, "set.claude.enabled", &ui.checkbox(&mut c.enabled, tr("set.claude.enabled")));
        egui::Grid::new("claude").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            mark(&focus, "set.claude.credentials", &ui.label(tr("set.claude.credentials")));
            ui.vertical(|ui| {
                let default = resolve_credentials_path("").display().to_string();
                ui.add(TextEdit::singleline(&mut c.credentials_path).hint_text(&default).desired_width(380.0));
                let path = resolve_credentials_path(&c.credentials_path);
                if path.exists() {
                    ui.label(RichText::new(format!("✔ {}", path.display())).small().color(Color32::from_rgb(0x6C, 0xCB, 0x5F)));
                } else {
                    ui.label(
                        RichText::new(format!("✖ {} — {}", path.display(), tr("claude.no_credentials")))
                            .small()
                            .color(Color32::from_rgb(0xE8, 0x8B, 0x6A)),
                    );
                }
            });
            ui.end_row();

            mark(&focus, "set.claude.interval", &ui.label(tr("set.claude.interval")));
            ui.add(Slider::new(&mut c.poll_interval_secs, 30..=900).suffix(" s"));
            ui.end_row();

            ui.label("");
            mark(&focus, "set.claude.countdown", &ui.checkbox(&mut c.show_reset_countdown, tr("set.claude.countdown")));
            ui.end_row();
        });
        ui.collapsing(tr("set.advanced"), |ui| {
            ui.horizontal(|ui| {
                mark(&focus, "set.claude.endpoint", &ui.label(tr("set.claude.endpoint")));
                ui.add(TextEdit::singleline(&mut c.usage_endpoint).desired_width(380.0));
            });
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let busy = self.claude_test.is_some();
            if ui.add_enabled(!busy, egui::Button::new(tr("set.claude.test"))).clicked() {
                let cfg = self.settings.claude.clone();
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx.send(collector::claude::fetch(&cfg).0);
                });
                self.claude_test = Some(rx);
            }
            if busy {
                ui.spinner();
            }
        });
        if let Some(r) = &self.claude_result {
            ui.add_space(4.0);
            if r.status == ClaudeStatus::Ok {
                if let Some(plan) = &r.subscription {
                    ui.label(trf("flyout.plan", &[plan]));
                }
                egui::Grid::new("claude_limits").striped(true).num_columns(3).show(ui, |ui| {
                    for l in &r.limits {
                        ui.monospace(&l.key);
                        ui.label(format!("{:.0}%", l.utilization));
                        ui.label(
                            l.seconds_to_reset(tm_core::time::now_unix())
                                .map(|s| trf("flyout.resets", &[&tm_core::format::duration(s)]))
                                .unwrap_or_default(),
                        );
                        ui.end_row();
                    }
                });
            } else {
                ui.label(
                    RichText::new(format!("{} {}", tr(r.status.key()), r.error.clone().unwrap_or_default()))
                        .color(Color32::from_rgb(0xE8, 0x8B, 0x6A)),
                );
            }
        }
    }

    fn hardware(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        let sources = self.sources.lock().unwrap().clone();
        let h = &mut self.settings.hardware;
        mark(&focus, "set.hw.gpu_sensors", &ui.checkbox(&mut h.gpu_sensors, tr("set.hw.gpu_sensors")));
        mark(&focus, "set.hw.thermal", &ui.checkbox(&mut h.thermal_zones, tr("set.hw.thermal")));
        mark(&focus, "set.hw.lhm", &ui.checkbox(&mut h.lhm_bridge, tr("set.hw.lhm")));
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("set.hw.lhm_hint")).weak());
            ui.hyperlink_to("LibreHardwareMonitor", "https://github.com/LibreHardwareMonitor/LibreHardwareMonitor");
        });
        ui.add_space(6.0);
        egui::Grid::new("hw").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            mark(&focus, "set.hw.interval", &ui.label(tr("set.hw.interval")));
            ui.add(Slider::new(&mut h.sensor_interval_ms, 500..=10_000).suffix(" ms").step_by(250.0));
            ui.end_row();

            mark(&focus, "set.hw.gpu_display", &ui.label(tr("set.hw.gpu_display")));
            ui.vertical(|ui| {
                for mode in GpuDisplay::ALL {
                    ui.radio_value(&mut h.gpu_display, mode, tr(mode.key()));
                }
            });
            ui.end_row();

            if h.gpu_display == GpuDisplay::Main {
                mark(&focus, "set.hw.gpu", &ui.label(tr("set.hw.gpu")));
                let label =
                    if h.gpu_adapter.is_empty() { tr("set.hw.gpu_most_memory").to_string() } else { h.gpu_adapter.clone() };
                ComboBox::from_id_salt("gpu_default").width(320.0).selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut h.gpu_adapter, String::new(), tr("set.hw.gpu_most_memory"));
                    for g in sources.iter().flat_map(|s| s.gpus.iter()) {
                        ui.selectable_value(&mut h.gpu_adapter, g.clone(), g);
                    }
                });
                ui.end_row();
            }
        });

        ui.add_space(10.0);
        mark(&focus, "set.hw.gpu_diag", &ui.heading(tr("set.hw.gpu_diag")));
        match &sources {
            None => {
                ui.spinner();
            }
            Some(s) => {
                egui::Grid::new("gpu_diag").striped(true).num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                    ui.strong("GPU");
                    ui.strong(tr("kind.gpu_usage"));
                    ui.strong("VRAM");
                    ui.strong("LUID");
                    ui.end_row();
                    for g in &s.gpu_diag.rows {
                        let name = if g.is_default { format!("★ {}", g.name) } else { g.name.clone() };
                        ui.label(name);
                        ui.label(g.usage.map(|u| format!("{u:.0}%")).unwrap_or_else(|| "—".into()));
                        ui.label(if g.vram_total > 0 {
                            format!("{} / {} MB", g.vram_used >> 20, g.vram_total >> 20)
                        } else {
                            format!("{} MB", g.vram_used >> 20)
                        });
                        ui.monospace(&g.luid);
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(tr("set.hw.copy_diag")).clicked() {
                        ui.ctx().copy_text(s.gpu_diag.text.clone());
                        self.flash(tr("set.hw.copied"));
                    }
                    if ui.button(tr("set.hw.refresh")).clicked() {
                        *self.sources.lock().unwrap() = None;
                        let sources = self.sources.clone();
                        let ctx = ui.ctx().clone();
                        std::thread::spawn(move || {
                            *sources.lock().unwrap() = Some(enumerate_sources());
                            ctx.request_repaint();
                        });
                    }
                });
            }
        }

        ui.add_space(10.0);
        mark(&focus, "set.hw.detected", &ui.heading(tr("set.hw.detected")));
        match &sources {
            None => {
                ui.spinner();
            }
            Some(s) => {
                ui.label(format!("GPU: {}", if s.gpus.is_empty() { "—".into() } else { s.gpus.join(", ") }));
                ui.label(format!("{}: {}", tr("set.hw.sensors"), s.sensors.len()));
                if s.sensors.is_empty() {
                    ui.label(RichText::new(tr("set.hw.no_lhm")).weak());
                } else {
                    ui.label(RichText::new(tr("set.hw.add_sensor_hint")).weak());
                    ScrollArea::vertical().id_salt("sensors").max_height(260.0).show(ui, |ui| {
                        for (id, label) in &s.sensors {
                            ui.horizontal(|ui| {
                                if ui.small_button("+").clicked() {
                                    let mut m = MetricConfig::new(MetricKind::Sensor);
                                    m.source = id.clone();
                                    self.settings.metrics.push(m);
                                    self.flash(tr("set.saved"));
                                }
                                ui.label(label);
                            });
                        }
                    });
                }
            }
        }
    }

    fn about(&mut self, ui: &mut egui::Ui) {
        let focus = self.focus.clone();
        ui.heading(format!("Taskbar Monitor {}", env!("CARGO_PKG_VERSION")));
        ui.add_space(6.0);
        egui::Grid::new("about").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
            mark(&focus, "set.about.mode", &ui.label(tr("set.about.mode")));
            ui.label(if self.paths.portable { tr("flyout.portable") } else { tr("flyout.installed") });
            ui.end_row();
            mark(&focus, "set.about.rights", &ui.label(tr("set.about.rights")));
            ui.label(if self.elevated { tr("flyout.admin") } else { tr("flyout.user") });
            ui.end_row();
            mark(&focus, "set.about.config", &ui.label(tr("set.about.config")));
            ui.monospace(self.paths.config_file().display().to_string());
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tr("menu.open_config")).clicked() {
                util::shell_open(&self.paths.config_file().display().to_string());
            }
            if ui.button(tr("menu.open_folder")).clicked() {
                util::shell_open(&self.paths.data_dir.display().to_string());
            }
        });
        if !self.elevated {
            ui.add_space(8.0);
            ui.label(RichText::new(tr("set.about.admin_hint")).weak());
            if ui.button(tr("menu.restart_admin")).clicked() {
                unsafe {
                    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
                    if let Ok(hwnd) = FindWindowW(&windows::core::HSTRING::from(MAIN_CLASS), windows::core::PCWSTR::null()) {
                        let _ = PostMessageW(Some(hwnd), WM_APP_RESTART_ELEVATED, Default::default(), Default::default());
                    }
                }
            }
        }
        ui.add_space(12.0);
        ui.hyperlink_to("GitHub: noartem/taskbar-monitor-widget", "https://github.com/noartem/taskbar-monitor-widget");

        ui.add_space(16.0);
        if self.confirm_reset {
            ui.horizontal(|ui| {
                mark(&focus, "set.reset.confirm", &ui.label(tr("set.reset.confirm")));
                if ui.button(tr("set.yes")).clicked() {
                    self.settings = Settings::default();
                    self.confirm_reset = false;
                }
                if ui.button(tr("set.no")).clicked() {
                    self.confirm_reset = false;
                }
            });
        } else if ui.button(tr("set.reset")).clicked() {
            self.confirm_reset = true;
        }
    }
}

/// A setting the search jumped to: highlighted for a moment and scrolled into view once.
#[derive(Clone, Default)]
struct Focus {
    key: Option<&'static str>,
    until: Option<Instant>,
    scroll: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Focus {
    fn new(key: &'static str) -> Self {
        Self {
            key: Some(key),
            until: Some(Instant::now() + Duration::from_millis(2500)),
            scroll: std::rc::Rc::new(std::cell::Cell::new(true)),
        }
    }

    fn active(&self) -> bool {
        self.until.is_some_and(|u| Instant::now() < u)
    }

    fn is(&self, key: &str) -> bool {
        self.active() && self.key == Some(key)
    }
}

/// Outlines the widget of the setting the search jumped to.
fn mark(focus: &Focus, key: &str, response: &egui::Response) {
    if focus.is(key) {
        let color = response.ctx.global_style().visuals.selection.bg_fill;
        response.ctx.layer_painter(response.layer_id).rect_stroke(
            response.rect.expand(4.0),
            4.0,
            egui::Stroke::new(2.0, color),
            egui::StrokeKind::Outside,
        );
        if focus.scroll.replace(false) {
            response.scroll_to_me(Some(egui::Align::Center));
        }
    }
}

/// Searchable settings: the label key of each and the section it lives in.
const SEARCH_INDEX: &[(Section, &str)] = &[
    (Section::General, "set.language"),
    (Section::General, "set.refresh"),
    (Section::General, "set.history"),
    (Section::General, "set.network_unit"),
    (Section::General, "set.left_click"),
    (Section::General, "set.double_click"),
    (Section::General, "set.middle_click"),
    (Section::General, "menu.autostart"),
    (Section::Appearance, "set.theme"),
    (Section::Appearance, "set.text_color"),
    (Section::Appearance, "set.background"),
    (Section::Appearance, "set.corner_radius"),
    (Section::Appearance, "set.font"),
    (Section::Appearance, "set.bold"),
    (Section::Appearance, "set.font_size"),
    (Section::Appearance, "set.label_opacity"),
    (Section::Appearance, "set.accent"),
    (Section::Appearance, "set.system_accent"),
    (Section::Appearance, "set.color_by_category"),
    (Section::Appearance, "set.thresholds"),
    (Section::Appearance, "set.use_thresholds"),
    (Section::Appearance, "set.warn"),
    (Section::Appearance, "set.crit"),
    (Section::Appearance, "set.padding"),
    (Section::Appearance, "set.tile_spacing"),
    (Section::Appearance, "set.graph_width"),
    (Section::Appearance, "set.bar_width"),
    (Section::Layout, "menu.layout"),
    (Section::Layout, "set.placement"),
    (Section::Layout, "set.offset"),
    (Section::Layout, "menu.rows"),
    (Section::Layout, "set.vertical_margin"),
    (Section::Layout, "set.monitor"),
    (Section::Layout, "set.secondary_margin"),
    (Section::Layout, "set.mode"),
    (Section::Layout, "menu.lock"),
    (Section::Layout, "set.hide_fullscreen"),
    (Section::Layout, "set.tray"),
    (Section::Layout, "set.tray.show"),
    (Section::Layout, "set.tray.tooltip"),
    (Section::Layout, "set.tray.metric_icon"),
    (Section::Claude, "set.claude.enabled"),
    (Section::Claude, "set.claude.credentials"),
    (Section::Claude, "set.claude.interval"),
    (Section::Claude, "set.claude.countdown"),
    (Section::Claude, "set.claude.endpoint"),
    (Section::Hardware, "set.hw.gpu_sensors"),
    (Section::Hardware, "set.hw.thermal"),
    (Section::Hardware, "set.hw.lhm"),
    (Section::Hardware, "set.hw.interval"),
    (Section::Hardware, "set.hw.gpu_display"),
    (Section::Hardware, "set.hw.gpu"),
    (Section::Hardware, "set.hw.gpu_diag"),
    (Section::About, "set.about.config"),
    (Section::About, "menu.restart_admin"),
    (Section::About, "set.reset"),
];

/// Matches `query` (case-insensitive, every word) against a label in both languages and its section.
fn search_matches(query: &str, key: &str, section: Section) -> bool {
    let haystack = [
        i18n::tr_in(key, i18n::Lang::En),
        i18n::tr_in(key, i18n::Lang::Ru),
        i18n::tr_in(section.key(), i18n::Lang::En),
        i18n::tr_in(section.key(), i18n::Lang::Ru),
    ]
    .join(" ")
    .to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(&word.to_lowercase()))
}

/// Search hits: settings plus metric kinds (which open the Metrics section).
fn search(query: &str) -> Vec<(Section, &'static str)> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(Section, &'static str)> =
        SEARCH_INDEX.iter().copied().filter(|(sec, key)| search_matches(query, key, *sec)).collect();
    for kind in MetricKind::ALL {
        let key = tr_key_static(kind);
        if search_matches(query, key, Section::Metrics) {
            hits.push((Section::Metrics, key));
        }
    }
    hits
}

/// `kind.<name>` as a `'static` key for the search index.
fn tr_key_static(kind: MetricKind) -> &'static str {
    match kind {
        MetricKind::CpuUsage => "kind.cpu_usage",
        MetricKind::CpuTemp => "kind.cpu_temp",
        MetricKind::RamUsage => "kind.ram_usage",
        MetricKind::GpuUsage => "kind.gpu_usage",
        MetricKind::GpuTemp => "kind.gpu_temp",
        MetricKind::VramUsage => "kind.vram_usage",
        MetricKind::NetDown => "kind.net_down",
        MetricKind::NetUp => "kind.net_up",
        MetricKind::NetTotal => "kind.net_total",
        MetricKind::DiskRead => "kind.disk_read",
        MetricKind::DiskWrite => "kind.disk_write",
        MetricKind::DiskActive => "kind.disk_active",
        MetricKind::DiskSpace => "kind.disk_space",
        MetricKind::ClaudeSession => "kind.claude_session",
        MetricKind::ClaudeWeekly => "kind.claude_weekly",
        MetricKind::ClaudeModel => "kind.claude_model",
        MetricKind::ClaudeExtra => "kind.claude_extra",
        MetricKind::Sensor => "kind.sensor",
    }
}

fn source_picker(ui: &mut egui::Ui, m: &mut MetricConfig, source_type: SourceType, sources: Option<&Sources>) {
    let options: Vec<(String, String)> = match (source_type, sources) {
        (SourceType::Gpu, Some(s)) => std::iter::once(("*".to_string(), tr("gpu.busiest").to_string()))
            .chain(s.gpus.iter().map(|g| (g.clone(), g.clone())))
            .collect(),
        (SourceType::Network, Some(s)) => std::iter::once(("*".to_string(), tr("set.source.all_adapters").to_string()))
            .chain(s.networks.iter().map(|(n, d)| (n.clone(), format!("{n} — {d}"))))
            .collect(),
        (SourceType::Disk, Some(s)) => s.disks.iter().map(|d| (d.clone(), d.clone())).collect(),
        (SourceType::Drive, Some(s)) => s.drives.iter().map(|d| (d.clone(), d.clone())).collect(),
        (SourceType::Sensor, Some(s)) => s.sensors.clone(),
        (SourceType::ClaudeLimit, _) => ["fable", "opus", "sonnet"].iter().map(|m| (m.to_string(), m.to_string())).collect(),
        _ => Vec::new(),
    };
    let current = if m.source.is_empty() {
        tr("set.auto").to_string()
    } else {
        options.iter().find(|(v, _)| *v == m.source).map(|(_, l)| l.clone()).unwrap_or_else(|| m.source.clone())
    };
    ComboBox::from_id_salt("source").width(300.0).selected_text(current).show_ui(ui, |ui| {
        if source_type != SourceType::Sensor {
            ui.selectable_value(&mut m.source, String::new(), tr("set.auto"));
        }
        for (value, label) in &options {
            ui.selectable_value(&mut m.source, value.clone(), label);
        }
    });
    ui.add(TextEdit::singleline(&mut m.source).hint_text(tr("set.source.custom")).desired_width(300.0));
    let hint = match source_type {
        SourceType::Network => tr("set.source.network_hint"),
        SourceType::ClaudeLimit => tr("set.source.claude_hint"),
        SourceType::Disk => tr("set.source.disk_hint"),
        SourceType::Gpu => tr("set.source.gpu_hint"),
        _ => "",
    };
    if !hint.is_empty() {
        ui.label(RichText::new(hint).small().weak());
    }
}

fn theme_key(t: ThemeMode) -> &'static str {
    match t {
        ThemeMode::System => "set.theme.system",
        ThemeMode::Light => "set.theme.light",
        ThemeMode::Dark => "set.theme.dark",
        ThemeMode::Custom => "set.theme.custom",
    }
}

fn placement_key(p: Placement) -> &'static str {
    match p {
        Placement::NearTray => "set.placement.tray",
        Placement::Left => "set.placement.left",
        Placement::Center => "set.placement.center",
    }
}

fn monitor_key(m: MonitorMode) -> &'static str {
    match m {
        MonitorMode::Primary => "set.monitor.primary",
        MonitorMode::All => "set.monitor.all",
        MonitorMode::Specific => "set.monitor.specific",
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.poll_jobs();
        if self.autostart_job.is_some() || self.claude_test.is_some() || self.focus.active() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let search_id = egui::Id::unique("settings_search");
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_id));
        }
        egui::Panel::left("nav").resizable(false).exact_size(190.0).show(ui, |ui| {
            ui.add_space(12.0);
            ui.heading("Taskbar Monitor");
            ui.add_space(10.0);
            let search_box =
                ui.add(TextEdit::singleline(&mut self.query).id(search_id).hint_text(tr("set.search")).desired_width(170.0));
            // Enter jumps to the first hit (a single-line edit loses focus on Enter).
            if search_box.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some((section, key)) = search(&self.query).first().copied()
            {
                self.jump_to(section, key);
            }
            ui.add_space(10.0);
            for s in Section::ALL {
                let selected = self.section == s && self.query.trim().is_empty();
                let text = RichText::new(tr(s.key())).size(15.0);
                if ui.add_sized([170.0, 30.0], egui::Button::selectable(selected, text)).clicked() {
                    self.section = s;
                    self.query.clear();
                }
            }
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let dirty = self.settings != self.saved;
                let text = match &self.status {
                    Some((t, at)) if at.elapsed() < Duration::from_secs(4) => t.clone(),
                    _ if dirty => tr("set.pending").to_string(),
                    _ => tr("set.live").to_string(),
                };
                ui.label(RichText::new(text).weak());
            });
        });
        let content_frame =
            egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin { left: 28, right: 20, top: 8, bottom: 8 });
        egui::CentralPanel::default().frame(content_frame).show(ui, |ui| {
            ui.add_space(8.0);
            if !self.query.trim().is_empty() {
                ui.heading(trf("set.search.results", &[self.query.trim()]));
                ui.add_space(8.0);
                ScrollArea::vertical().id_salt("search").auto_shrink([false, false]).show(ui, |ui| self.search_results(ui));
                return;
            }
            ui.heading(tr(self.section.key()));
            ui.add_space(8.0);
            ScrollArea::vertical().id_salt(self.section.key()).auto_shrink([false, false]).show(ui, |ui| match self.section {
                Section::General => self.general(ui),
                Section::Appearance => self.appearance(ui),
                Section::Layout => self.layout(ui),
                Section::Metrics => self.metrics(ui),
                Section::Claude => self.claude(ui),
                Section::Hardware => self.hardware(ui),
                Section::About => self.about(ui),
            });
        });

        if self.applied_theme != Some(self.settings.appearance.theme) {
            self.apply_theme(&ctx);
        }
        self.sync_file(&ctx);
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let candidates = [("segoe", "segoeui.ttf"), ("segoe_symbol", "seguisym.ttf")];
    let mut added = Vec::new();
    for (name, file) in candidates {
        if let Ok(bytes) = std::fs::read(PathBuf::from(&windir).join("Fonts").join(file)) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            added.push(name);
        }
    }
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        for (i, name) in added.iter().enumerate() {
            // Segoe UI first, the symbol font as the first fallback.
            let pos = if i == 0 { 0 } else { 1.min(list.len()) };
            list.insert(pos, (*name).to_string());
        }
    }
    ctx.set_fonts(fonts);
}

/// The executable's icon as RGBA for the window title bar.
fn exe_icon() -> Option<egui::IconData> {
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, GetIconInfo, HICON, ICONINFO, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
    };
    const SIZE: i32 = 64;
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let handle = LoadImageW(
            Some(module.into()),
            windows::core::PCWSTR(std::ptr::dangling::<u16>()),
            IMAGE_ICON,
            SIZE,
            SIZE,
            LR_DEFAULTCOLOR,
        )
        .ok()?;
        let icon = HICON(handle.0);
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, info.hbmColor, 0, SIZE as u32, Some(pixels.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        let _ = DestroyIcon(icon);
        if lines == 0 {
            return None;
        }
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2); // BGRA → RGBA
        }
        Some(egui::IconData { rgba: pixels, width: SIZE as u32, height: SIZE as u32 })
    }
}

pub fn run(paths: AppPaths, _passthrough: Vec<String>) -> i32 {
    // One settings window at a time.
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow};
        if let Ok(hwnd) = FindWindowW(windows::core::PCWSTR::null(), &windows::core::HSTRING::from(SETTINGS_TITLE)) {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
            return 0;
        }
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(SETTINGS_TITLE)
        .with_inner_size([960.0, 680.0])
        .with_min_inner_size([760.0, 520.0]);
    if let Some(icon) = exe_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    match eframe::run_native(SETTINGS_TITLE, options, Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, paths))))) {
        Ok(()) => 0,
        Err(e) => {
            util::error_box(&format!("Settings window failed: {e}"));
            1
        }
    }
}
