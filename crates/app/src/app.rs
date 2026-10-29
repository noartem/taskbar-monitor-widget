//! The tray application: owns settings, the collector, widget windows, the flyout and the tray icon.
//! Everything UI-related runs on the main thread; window procedures forward into a thread-local `App`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use tm_core::color::Palette;
use tm_core::config::{self, AppPaths, Autostart, ClickAction, GpuDisplay, LoadOutcome, MetricConfig, Settings, TrayIconMode};
use tm_core::history::History;
use tm_core::i18n::{self, tr, trf};
use tm_core::metrics::DataSources;
use tm_core::presets;
use tm_core::resolver::{self, Reading};
use tm_core::snapshot::Snapshot;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetDoubleClickTime, ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_SYSTEM_FOREGROUND, FindWindowW, GetCursorPos, GetMessageW, GetWindowThreadProcessId, IDC_ARROW, KillTimer, LoadCursorW,
    MSG, OBJID_WINDOW, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SW_RESTORE, SetForegroundWindow,
    SetTimer, ShowWindow, TranslateMessage, WA_INACTIVE, WINDOW_EX_STYLE, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WM_ACTIVATE, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_DWMCOLORIZATIONCOLORCHANGED, WM_KEYDOWN,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER,
    WNDCLASSEXW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR};

use crate::collector::{Collector, Requirements};
use crate::flyout::{self, FLYOUT_CLASS, Flyout, FlyoutButton, FlyoutData};
use crate::render::Gfx;
use crate::taskbar;
use crate::tiles::{self, TileInput};
use crate::tray::{self, Tray};
use crate::widget::{WIDGET_CLASS, Widget};
use crate::{autostart, elevation, theme, util};

pub const MAIN_CLASS: &str = "TaskbarMonitor.Main";
pub const SETTINGS_TITLE: &str = "Taskbar Monitor — Settings";
pub const WM_APP_SNAPSHOT: u32 = WM_APP + 1;
pub const WM_APP_SHOW_SETTINGS: u32 = WM_APP + 2;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_RESTART_ELEVATED: u32 = WM_APP + 4;
const WM_APP_REASSERT: u32 = WM_APP + 5;
const WM_APP_REPOSITION: u32 = WM_APP + 6;
const WM_APP_AUTOSTART_DONE: u32 = WM_APP + 7;
const TIMER_HOUSEKEEPING: usize = 1;
const TIMER_CLICK: usize = 2;
const NIN_SELECT: u32 = 0x400;
const NIN_KEYSELECT: u32 = 0x401;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static MAIN_HWND: Cell<isize> = const { Cell::new(0) };
    static TASKBAR_HWNDS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` on the app unless it is already borrowed (re-entrant message during a modal loop).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut guard| guard.as_mut().map(f)))
}

pub struct Options {
    pub paths: AppPaths,
    /// Arguments to keep when restarting (e.g. `--portable`).
    pub passthrough: Vec<String>,
}

struct Drag {
    widget: usize,
    start: POINT,
    start_offset: f32,
    moved: bool,
}

struct PendingClick {
    widget: Option<usize>,
}

struct App {
    paths: AppPaths,
    passthrough: Vec<String>,
    settings: Settings,
    config_mtime: Option<SystemTime>,
    main: HWND,
    gfx: Gfx,
    palette: Palette,
    collector: Collector,
    snapshot: Snapshot,
    /// `settings.metrics` with "all GPUs" expanded; `readings` is parallel to it.
    metrics: Vec<MetricConfig>,
    readings: Vec<Reading>,
    histories: HashMap<String, History>,
    widgets: Vec<Widget>,
    user_hidden: bool,
    tray: Option<Tray>,
    flyout: Option<Flyout>,
    elevated: bool,
    taskbar_created: u32,
    hooks: Vec<HWINEVENTHOOK>,
    settings_process: Option<std::process::Child>,
    drag: Option<Drag>,
    pending_click: Option<PendingClick>,
    ignore_next_up: bool,
    tick: u64,
    last_reposition: Instant,
}

pub fn run(opts: Options) -> i32 {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return 1 };
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for (name, proc_, style) in [
            (MAIN_CLASS, main_proc as WndProc, Default::default()),
            (WIDGET_CLASS, widget_proc as WndProc, CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW),
            (FLYOUT_CLASS, flyout_proc as WndProc, CS_HREDRAW | CS_VREDRAW),
        ] {
            let class = HSTRING::from(name);
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style,
                lpfnWndProc: Some(proc_),
                hInstance: instance.into(),
                hCursor: cursor,
                hbrBackground: HBRUSH::default(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }

        // A hidden top-level window (not message-only) so it receives TaskbarCreated and theme broadcasts.
        let Ok(main) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            &HSTRING::from(MAIN_CLASS),
            &HSTRING::from("Taskbar Monitor"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            util::error_box("Cannot create the main window.");
            return 1;
        };
        MAIN_HWND.set(main.0 as isize);

        let app = match App::new(opts, main) {
            Ok(app) => app,
            Err(e) => {
                util::error_box(&format!("Taskbar Monitor failed to start: {e}"));
                return 1;
            }
        };
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        with_app(|app| app.after_start());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let app = APP.with(|cell| cell.borrow_mut().take());
        drop(app);
    }
    0
}

type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(Some(r)) = with_app(|app| app.on_main(msg, wp, lp)) {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn widget_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(Some(r)) = with_app(|app| app.on_widget(hwnd, msg, wp, lp)) {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn flyout_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(Some(r)) = with_app(|app| app.on_flyout(msg, wp, lp)) {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _: i32, _: u32, _: u32) {
    let main = util::hwnd_from_isize(MAIN_HWND.get());
    let msg = if event == EVENT_SYSTEM_FOREGROUND {
        WM_APP_REASSERT
    } else if id_object == OBJID_WINDOW.0 && TASKBAR_HWNDS.with(|t| t.borrow().contains(&(hwnd.0 as isize))) {
        WM_APP_REPOSITION
    } else {
        return;
    };
    unsafe {
        let _ = PostMessageW(Some(main), msg, WPARAM(0), LPARAM(0));
    }
}

fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl App {
    fn new(opts: Options, main: HWND) -> windows::core::Result<Self> {
        let config_path = opts.paths.config_file();
        let (settings, outcome) = config::load(&config_path);
        i18n::init(&settings.general.language, &util::ui_language());
        if let LoadOutcome::Recovered { backup, error } = &outcome {
            util::error_box(&trf("dlg.config_broken", &[&backup.display().to_string(), error]));
        }
        crate::log!(
            "started: portable={} elevated={} config={}",
            opts.paths.portable,
            elevation::is_elevated(),
            config_path.display()
        );

        let gfx = Gfx::new()?;
        let palette = Palette::build(&settings.appearance, theme::taskbar_is_dark(), theme::accent_color());
        theme::set_menu_theme(palette.dark);
        let collector = Collector::start(requirements(&settings), main.0 as isize);
        let flyout = Flyout::create(&gfx);
        let taskbar_created = unsafe { RegisterWindowMessageW(&HSTRING::from("TaskbarCreated")) };

        Ok(Self {
            paths: opts.paths,
            passthrough: opts.passthrough,
            config_mtime: file_mtime(&config_path),
            settings,
            main,
            gfx,
            palette,
            collector,
            snapshot: Snapshot::default(),
            metrics: Vec::new(),
            readings: Vec::new(),
            histories: HashMap::new(),
            widgets: Vec::new(),
            user_hidden: false,
            tray: None,
            flyout,
            elevated: elevation::is_elevated(),
            taskbar_created,
            hooks: Vec::new(),
            settings_process: None,
            drag: None,
            pending_click: None,
            ignore_next_up: false,
            tick: 0,
            last_reposition: Instant::now(),
        })
    }

    fn after_start(&mut self) {
        if self.settings.tray.show_icon {
            let mut tray = Tray::new(self.main);
            tray.add();
            self.tray = Some(tray);
        }
        // Keep the Run key pointing at this exe if the app was moved (no admin rights needed).
        if self.settings.general.autostart == Autostart::User {
            let _ = autostart::set(Autostart::User);
        }
        self.rebuild_widgets();
        self.install_hooks();
        self.recompute_readings();
        self.render();
        unsafe {
            SetTimer(Some(self.main), TIMER_HOUSEKEEPING, 1000, None);
        }
    }

    // ---- settings ----------------------------------------------------------------------------------------

    fn save_settings(&mut self) {
        let path = self.paths.config_file();
        if let Err(e) = config::save(&path, &self.settings) {
            crate::log!("saving settings failed: {e}");
        }
        self.config_mtime = file_mtime(&path);
    }

    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        if old.general.language != self.settings.general.language {
            i18n::init(&self.settings.general.language, &util::ui_language());
        }
        self.collector.update(requirements(&self.settings));
        let cap = self.settings.general.history_length as usize;
        for h in self.histories.values_mut() {
            h.resize(cap);
        }
        self.palette = Palette::build(&self.settings.appearance, theme::taskbar_is_dark(), theme::accent_color());
        theme::set_menu_theme(self.palette.dark);

        match (self.settings.tray.show_icon, self.tray.is_some()) {
            (true, false) => {
                let mut t = Tray::new(self.main);
                t.add();
                self.tray = Some(t);
            }
            (false, true) => self.tray = None,
            _ => {}
        }
        if self.settings.tray.icon_mode == TrayIconMode::AppIcon
            && old.tray.icon_mode == TrayIconMode::Metric
            && let Some(t) = &mut self.tray
        {
            t.set_metric_icon(&self.gfx, None, &self.palette, 1.0);
        }

        if old.layout.embed != self.settings.layout.embed
            || old.layout.monitor != self.settings.layout.monitor
            || old.layout.monitor_index != self.settings.layout.monitor_index
        {
            self.widgets.clear();
        }
        self.rebuild_widgets();
        self.recompute_readings();
        self.render();
    }

    fn check_config_file(&mut self) {
        let path = self.paths.config_file();
        let mtime = file_mtime(&path);
        if mtime.is_none() || mtime == self.config_mtime {
            return;
        }
        self.config_mtime = mtime;
        match std::fs::read_to_string(&path).map(|t| Settings::from_toml(&t)) {
            Ok(Ok(s)) => {
                crate::log!("config reloaded");
                self.apply_settings(s);
            }
            Ok(Err(e)) => crate::log!("config not applied, parse error: {e}"),
            Err(e) => crate::log!("config not readable: {e}"),
        }
    }

    // ---- data ---------------------------------------------------------------------------------------------

    fn on_snapshot(&mut self) {
        match self.collector.take_snapshot() {
            Some(snap) => {
                self.snapshot = snap;
                self.recompute_readings();
                let cap = self.settings.general.history_length as usize;
                let mut pushed = std::collections::HashSet::new();
                for (m, r) in self.metrics.iter().zip(&self.readings) {
                    let key = flyout::history_key(m);
                    if pushed.insert(key.clone()) {
                        self.histories.entry(key).or_insert_with(|| History::new(cap)).push(r.value);
                    }
                }
            }
            None => {
                // Subscription-only update.
                {
                    let st = self.collector.shared.state.lock().unwrap();
                    self.snapshot.claude = st.claude.clone();
                    self.snapshot.codex = st.codex.clone();
                    self.snapshot.ollama = st.ollama.clone();
                }
                self.recompute_readings();
            }
        }
        self.render();
    }

    fn recompute_readings(&mut self) {
        self.metrics = resolver::expand_metrics(&self.settings.metrics, &self.snapshot, &self.settings.hardware);
        self.readings = self.metrics.iter().map(|m| resolver::resolve(m, &self.snapshot, &self.settings)).collect();
    }

    // ---- widgets ------------------------------------------------------------------------------------------

    fn rebuild_widgets(&mut self) {
        let wanted = taskbar::select(taskbar::enumerate(), &self.settings.layout);
        let same = wanted.len() == self.widgets.len() && wanted.iter().zip(&self.widgets).all(|(t, w)| t.hwnd == w.taskbar.hwnd);
        if same {
            for (t, w) in wanted.into_iter().zip(&mut self.widgets) {
                w.taskbar = t;
            }
        } else {
            self.widgets.clear();
            for t in wanted {
                crate::log!(
                    "taskbar: rect=({},{})-({},{}) edge={:?} tray={:?} scale={} hidden={}",
                    t.rect.left,
                    t.rect.top,
                    t.rect.right,
                    t.rect.bottom,
                    t.edge,
                    t.tray.map(|r| (r.left, r.top, r.right, r.bottom)),
                    t.scale,
                    t.hidden
                );
                if let Some(w) = Widget::create(t, self.settings.layout.embed, &self.gfx) {
                    self.widgets.push(w);
                }
            }
        }
        TASKBAR_HWNDS.with(|list| *list.borrow_mut() = self.widgets.iter().map(|w| w.taskbar.hwnd.0 as isize).collect());
    }

    fn widget_visible(&self, w: &Widget) -> bool {
        !(self.user_hidden
            || w.taskbar.hidden
            || self.settings.layout.hide_on_fullscreen && taskbar::fullscreen_on(w.taskbar.monitor, w.taskbar.monitor_rect))
    }

    fn render(&mut self) {
        let inputs: Vec<TileInput> = self
            .metrics
            .iter()
            .zip(&self.readings)
            .map(|(m, r)| TileInput { config: m, reading: r, history: self.histories.get(&flyout::history_key(m)) })
            .collect();

        let visible: Vec<bool> = self.widgets.iter().map(|w| self.widget_visible(w)).collect();
        for (w, show) in self.widgets.iter_mut().zip(visible) {
            if show {
                w.render(&self.gfx, &inputs, &self.settings, &self.palette);
            }
            w.place(&self.settings, show);
            if let Some((m, r)) = w.hover.and_then(|i| self.metrics.get(i).zip(self.readings.get(i))) {
                w.set_tooltip(&tooltip_text(m, r));
            }
        }

        if let Some(tray) = &mut self.tray {
            if self.settings.tray.show_tooltip {
                tray.set_tooltip(&tray_tooltip(&inputs));
            }
            if self.settings.tray.icon_mode == TrayIconMode::Metric {
                let kind = self.settings.tray.icon_metric;
                let config = self
                    .settings
                    .metrics
                    .iter()
                    .find(|m| m.kind == kind)
                    .cloned()
                    .unwrap_or_else(|| tm_core::config::MetricConfig::new(kind));
                let reading = resolver::resolve(&config, &self.snapshot, &self.settings);
                let input =
                    TileInput { config: &config, reading: &reading, history: self.histories.get(&flyout::history_key(&config)) };
                let text = if reading.available {
                    reading.value_text.trim_end_matches('%').split(' ').next().unwrap_or("").to_string()
                } else {
                    "–".into()
                };
                let color = tiles::base_color(&config, &self.palette);
                let color = if reading.uses_thresholds {
                    self.palette.threshold(color, reading.value, &self.settings.appearance)
                } else {
                    color
                };
                let scale = self.widgets.first().map(|w| w.taskbar.scale).unwrap_or(1.0);
                tray.set_metric_icon(&self.gfx, Some((&text, tiles::fraction(&input) as f32, color)), &self.palette, scale);
            }
        }

        if let Some(f) = &mut self.flyout
            && f.visible
        {
            let data = FlyoutData {
                settings: &self.settings,
                metrics: &self.metrics,
                readings: &self.readings,
                histories: &self.histories,
                snapshot: &self.snapshot,
                palette: &self.palette,
                elevated: self.elevated,
                portable: self.paths.portable,
            };
            f.refresh(&self.gfx, &data);
        }
    }

    /// Cheap path for taskbar moves/auto-hide animation: refresh geometry and move windows without redrawing.
    fn reposition(&mut self) {
        let mut needs_render = false;
        for w in &mut self.widgets {
            if let Some(fresh) = taskbar::refresh(&w.taskbar) {
                if fresh.thickness() != w.taskbar.thickness() || fresh.scale != w.taskbar.scale || fresh.edge != w.taskbar.edge {
                    needs_render = true;
                }
                w.taskbar = fresh;
            }
        }
        if needs_render {
            self.render();
            return;
        }
        let visible: Vec<bool> = self.widgets.iter().map(|w| self.widget_visible(w)).collect();
        for (w, show) in self.widgets.iter_mut().zip(visible) {
            w.place(&self.settings, show);
        }
        self.last_reposition = Instant::now();
    }

    fn install_hooks(&mut self) {
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
        unsafe {
            let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
            let fg = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(win_event), 0, 0, flags);
            if !fg.is_invalid() {
                self.hooks.push(fg);
            }
            if let Some(w) = self.widgets.first() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(w.taskbar.hwnd, Some(&mut pid));
                if pid != 0 {
                    let loc = SetWinEventHook(
                        EVENT_OBJECT_LOCATIONCHANGE,
                        EVENT_OBJECT_LOCATIONCHANGE,
                        None,
                        Some(win_event),
                        pid,
                        0,
                        flags,
                    );
                    if !loc.is_invalid() {
                        self.hooks.push(loc);
                    }
                }
            }
        }
    }

    // ---- actions ------------------------------------------------------------------------------------------

    fn do_action(&mut self, action: ClickAction, widget: Option<usize>) {
        match action {
            ClickAction::None => {}
            ClickAction::Flyout => self.toggle_flyout(widget),
            ClickAction::Settings => self.open_settings(),
            ClickAction::ToggleWidget => {
                self.user_hidden = !self.user_hidden;
                self.render();
            }
            ClickAction::TaskManager => util::open_task_manager(),
        }
    }

    fn toggle_flyout(&mut self, widget: Option<usize>) {
        let Some(f) = &mut self.flyout else { return };
        if f.visible {
            f.hide();
            return;
        }
        // A click on the tray icon first deactivates (and hides) the flyout; do not immediately reopen it.
        if widget.is_none() && f.hidden_at.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
            return;
        }
        let (tb, anchor) = match widget.and_then(|i| self.widgets.get(i)) {
            Some(w) => (w.taskbar.clone(), w.screen_rect()),
            None => {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                let tb = self.widgets.first().map(|w| w.taskbar.clone()).or_else(|| taskbar::enumerate().into_iter().next());
                let Some(tb) = tb else { return };
                (tb, RECT { left: pt.x, top: pt.y, right: pt.x, bottom: pt.y })
            }
        };
        let data = FlyoutData {
            settings: &self.settings,
            metrics: &self.metrics,
            readings: &self.readings,
            histories: &self.histories,
            snapshot: &self.snapshot,
            palette: &self.palette,
            elevated: self.elevated,
            portable: self.paths.portable,
        };
        f.show(&self.gfx, &data, tb, anchor);
    }

    fn open_settings(&mut self) {
        if let Some(child) = &mut self.settings_process
            && matches!(child.try_wait(), Ok(None))
        {
            unsafe {
                if let Ok(hwnd) = FindWindowW(PCWSTR::null(), &HSTRING::from(SETTINGS_TITLE)) {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
            return;
        }
        let mut args: Vec<&str> = vec!["--settings"];
        args.extend(self.passthrough.iter().map(String::as_str));
        match util::spawn_self(&args) {
            Ok(child) => self.settings_process = Some(child),
            Err(e) => util::error_box(&format!("Cannot open settings: {e}")),
        }
    }

    fn restart_elevated(&mut self) {
        if self.elevated || !util::confirm(tr("dlg.restart_admin")) {
            return;
        }
        let pid = std::process::id().to_string();
        let mut args: Vec<&str> = vec!["--wait-pid", &pid];
        args.extend(self.passthrough.iter().map(String::as_str));
        match elevation::run_self_elevated(&args, false) {
            Ok(elevation::ElevatedRun::Cancelled) => {}
            Ok(_) => self.exit(),
            Err(e) => util::error_box(&e.message()),
        }
    }

    fn set_autostart(&mut self, mode: Autostart) {
        let main = self.main.0 as isize;
        std::thread::spawn(move || {
            let ok = match autostart::set_with_elevation(mode) {
                Ok(done) => done,
                Err(e) => {
                    crate::log!("autostart change failed: {e}");
                    false
                }
            };
            unsafe {
                let _ = PostMessageW(
                    Some(util::hwnd_from_isize(main)),
                    WM_APP_AUTOSTART_DONE,
                    WPARAM(mode as usize),
                    LPARAM(ok as isize),
                );
            }
        });
    }

    fn exit(&mut self) {
        if let Some(f) = &mut self.flyout {
            f.hide();
        }
        self.tray = None;
        self.widgets.clear();
        unsafe { PostQuitMessage(0) };
    }

    fn on_menu(&mut self) {
        let Some(tray) = &self.tray else {
            // Without a tray icon the menu is still reachable from the widget; build a temporary one.
            let t = Tray::new(self.main);
            let cmd = t.show_menu(&self.settings, !self.user_hidden, self.elevated, autostart::current());
            self.on_command(cmd);
            return;
        };
        let cmd = tray.show_menu(&self.settings, !self.user_hidden, self.elevated, autostart::current());
        self.on_command(cmd);
    }

    fn on_command(&mut self, cmd: u32) {
        match cmd {
            tray::CMD_DETAILS => self.toggle_flyout(None),
            tray::CMD_SETTINGS => self.open_settings(),
            tray::CMD_TOGGLE_WIDGET => {
                self.user_hidden = !self.user_hidden;
                self.render();
            }
            tray::CMD_LOCK => {
                self.settings.layout.lock_position = !self.settings.layout.lock_position;
                self.save_settings();
            }
            c if (tray::CMD_PRESET_BASE..tray::CMD_PRESET_BASE + presets::ALL.len() as u32).contains(&c) => {
                let mut s = self.settings.clone();
                presets::apply(&mut s, &presets::ALL[(c - tray::CMD_PRESET_BASE) as usize]);
                self.apply_settings(s);
                self.save_settings();
            }
            c if (tray::CMD_ROWS_BASE + 1..=tray::CMD_ROWS_BASE + 4).contains(&c) => {
                let mut s = self.settings.clone();
                s.layout.rows = c - tray::CMD_ROWS_BASE;
                self.apply_settings(s);
                self.save_settings();
            }
            c if (tray::CMD_GPU_BASE..tray::CMD_GPU_BASE + GpuDisplay::ALL.len() as u32).contains(&c) => {
                let mut s = self.settings.clone();
                s.hardware.gpu_display = GpuDisplay::ALL[(c - tray::CMD_GPU_BASE) as usize];
                self.apply_settings(s);
                self.save_settings();
            }
            tray::CMD_AUTOSTART_OFF => self.set_autostart(Autostart::Off),
            tray::CMD_AUTOSTART_USER => self.set_autostart(Autostart::User),
            tray::CMD_AUTOSTART_ELEVATED if self.elevated || util::confirm(tr("dlg.elevated_autostart")) => {
                self.set_autostart(Autostart::Elevated);
            }
            tray::CMD_RESTART_ADMIN => self.restart_elevated(),
            tray::CMD_REFRESH_USAGE => self.collector.refresh_subscriptions(),
            tray::CMD_OPEN_CONFIG => util::shell_open(&self.paths.config_file().display().to_string()),
            tray::CMD_OPEN_FOLDER => util::shell_open(&self.paths.data_dir.display().to_string()),
            tray::CMD_EXIT => self.exit(),
            _ => {}
        }
    }

    // ---- window messages ----------------------------------------------------------------------------------

    fn on_main(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_APP_SNAPSHOT => self.on_snapshot(),
            WM_APP_TRAY => match util::loword(lp.0 as u32) as u32 {
                NIN_SELECT | NIN_KEYSELECT => self.do_action(self.settings.general.left_click, None),
                WM_CONTEXTMENU | WM_RBUTTONUP => self.on_menu(),
                WM_MBUTTONUP => self.do_action(self.settings.general.middle_click, None),
                _ => {}
            },
            WM_APP_SHOW_SETTINGS => self.open_settings(),
            WM_APP_RESTART_ELEVATED => self.restart_elevated(),
            WM_APP_REASSERT => {
                let fullscreen_changed = self.widgets.iter().any(|w| w.visible != self.widget_visible(w));
                if fullscreen_changed {
                    self.reposition();
                }
                for w in &self.widgets {
                    w.raise();
                }
            }
            WM_APP_REPOSITION => {
                // Throttle bursts of location events during auto-hide animations.
                if self.last_reposition.elapsed() >= Duration::from_millis(15) {
                    self.reposition();
                }
            }
            WM_APP_AUTOSTART_DONE => {
                let mode = match wp.0 {
                    1 => Autostart::User,
                    2 => Autostart::Elevated,
                    _ => Autostart::Off,
                };
                if lp.0 != 0 {
                    self.settings.general.autostart = mode;
                    self.save_settings();
                }
            }
            WM_TIMER if wp.0 == TIMER_HOUSEKEEPING => {
                self.tick += 1;
                self.check_config_file();
                if self.tick.is_multiple_of(5) {
                    let before = self.widgets.len();
                    self.rebuild_widgets();
                    if before != self.widgets.len() {
                        self.install_hooks();
                        self.render();
                        return Some(LRESULT(0));
                    }
                }
                self.reposition();
            }
            WM_TIMER if wp.0 == TIMER_CLICK => {
                unsafe {
                    let _ = KillTimer(Some(self.main), TIMER_CLICK);
                }
                if let Some(p) = self.pending_click.take() {
                    self.do_action(self.settings.general.left_click, p.widget);
                }
            }
            WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
                self.palette = Palette::build(&self.settings.appearance, theme::taskbar_is_dark(), theme::accent_color());
                theme::set_menu_theme(self.palette.dark);
                self.rebuild_widgets();
                self.render();
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                self.widgets.clear();
                self.rebuild_widgets();
                self.install_hooks();
                self.render();
            }
            WM_CLOSE => self.exit(),
            m if m == self.taskbar_created && m != 0 => {
                crate::log!("taskbar re-created (Explorer restart)");
                if let Some(t) = &mut self.tray {
                    t.add();
                }
                self.widgets.clear();
                self.rebuild_widgets();
                self.install_hooks();
                self.render();
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn widget_index(&self, hwnd: HWND) -> Option<usize> {
        self.widgets.iter().position(|w| w.hwnd == hwnd)
    }

    fn on_widget(&mut self, hwnd: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let idx = self.widget_index(hwnd)?;
        match msg {
            WM_MOUSEMOVE => {
                if let Some(d) = &mut self.drag {
                    let mut pt = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut pt);
                    }
                    let w = &self.widgets[d.widget];
                    let delta = if w.taskbar.horizontal() { pt.x - d.start.x } else { pt.y - d.start.y };
                    if delta.abs() > 3 {
                        d.moved = true;
                    }
                    if d.moved {
                        let sign =
                            if self.settings.layout.placement == tm_core::config::Placement::NearTray { -1.0 } else { 1.0 };
                        self.settings.layout.offset_x = d.start_offset + sign * delta as f32 / w.taskbar.scale;
                        let visible: Vec<bool> = self.widgets.iter().map(|w| self.widget_visible(w)).collect();
                        for (w, show) in self.widgets.iter_mut().zip(visible) {
                            w.place(&self.settings, show);
                        }
                    }
                    return Some(LRESULT(0));
                }
                let (x, y) = util::point_from_lparam(lp.0);
                let hover = self.widgets[idx].hit(x, y);
                if hover != self.widgets[idx].hover {
                    self.widgets[idx].hover = hover;
                    let text = hover
                        .and_then(|i| self.metrics.get(i).zip(self.readings.get(i)))
                        .map(|(m, r)| tooltip_text(m, r))
                        .unwrap_or_default();
                    self.widgets[idx].set_tooltip(&text);
                }
            }
            WM_LBUTTONDOWN => {
                if !self.settings.layout.lock_position {
                    let mut pt = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut pt);
                        SetCapture(hwnd);
                    }
                    self.drag = Some(Drag { widget: idx, start: pt, start_offset: self.settings.layout.offset_x, moved: false });
                }
            }
            WM_LBUTTONUP => {
                let dragged = match self.drag.take() {
                    Some(d) => {
                        unsafe {
                            let _ = ReleaseCapture();
                        }
                        d.moved
                    }
                    None => false,
                };
                if dragged {
                    self.save_settings();
                } else if self.ignore_next_up {
                    self.ignore_next_up = false;
                } else if self.settings.general.double_click == ClickAction::None {
                    self.do_action(self.settings.general.left_click, Some(idx));
                } else {
                    self.pending_click = Some(PendingClick { widget: Some(idx) });
                    unsafe {
                        SetTimer(Some(self.main), TIMER_CLICK, GetDoubleClickTime(), None);
                    }
                }
            }
            WM_LBUTTONDBLCLK => {
                unsafe {
                    let _ = KillTimer(Some(self.main), TIMER_CLICK);
                }
                self.pending_click = None;
                self.ignore_next_up = true;
                self.do_action(self.settings.general.double_click, Some(idx));
            }
            WM_RBUTTONUP => self.on_menu(),
            WM_MBUTTONUP => self.do_action(self.settings.general.middle_click, Some(idx)),
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn on_flyout(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let f = self.flyout.as_mut()?;
        match msg {
            WM_ACTIVATE if util::loword(wp.0 as u32) as u32 == WA_INACTIVE => f.hide(),
            WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => f.hide(),
            WM_LBUTTONUP => {
                let (x, y) = util::point_from_lparam(lp.0);
                match f.button_at(x, y) {
                    Some(FlyoutButton::Settings) => {
                        f.hide();
                        self.open_settings();
                    }
                    Some(FlyoutButton::Close) => f.hide(),
                    None => {}
                }
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

impl Drop for App {
    fn drop(&mut self) {
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
        self.tray = None;
        self.widgets.clear();
        self.collector.stop();
    }
}

/// Everything shown anywhere needs collecting; the details view keeps its history warm.
pub fn requirements(settings: &Settings) -> Requirements {
    let mut sources = DataSources::from_kinds(settings.required_kinds(true));
    if !settings.claude.enabled {
        sources = DataSources(sources.0 & !DataSources::CLAUDE.0);
    }
    if !settings.codex.enabled {
        sources = DataSources(sources.0 & !DataSources::CODEX.0);
    }
    if !settings.ollama.enabled {
        sources = DataSources(sources.0 & !DataSources::OLLAMA.0);
    }
    Requirements {
        sources,
        interval_ms: settings.general.refresh_interval_ms,
        hardware: settings.hardware.clone(),
        claude: settings.claude.clone(),
        codex: settings.codex.clone(),
        ollama: settings.ollama.clone(),
    }
}

fn tooltip_text(config: &tm_core::config::MetricConfig, r: &Reading) -> String {
    let name =
        if config.label.is_empty() { tr(&format!("kind.{}", config.kind.key())).to_string() } else { config.label.clone() };
    let mut lines = vec![format!("{name}: {}", r.value_text)];
    if let Some(secs) = r.resets_in {
        lines.push(trf("flyout.resets", &[&tm_core::format::duration(secs)]));
    } else if let Some(d) = &r.detail {
        lines.push(d.clone());
    }
    if let Some(s) = &r.source_name
        && !lines.iter().any(|l| l.contains(s.as_str()))
    {
        lines.push(s.clone());
    }
    if let Some(status) = r.status {
        lines.push(tr(status).to_string());
    }
    lines.join("\n")
}

/// Short summary for the tray tooltip (Windows limits it to 127 characters).
fn tray_tooltip(inputs: &[TileInput]) -> String {
    let mut out = String::from("Taskbar Monitor");
    for t in inputs.iter().filter(|t| t.config.show_in_widget && t.reading.available) {
        let part = format!("\n{} {}", t.reading.label, t.reading.value_text);
        if out.chars().count() + part.chars().count() > 127 {
            break;
        }
        out.push_str(&part);
    }
    out
}
