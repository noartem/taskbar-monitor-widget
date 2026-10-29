//! Taskbar Monitor: a Windows taskbar widget for system metrics and Claude/Codex/Ollama subscription usage.
//!
//! Command line:
//!   (none) | --autostart      run the widget (single instance)
//!   --settings                open the settings window
//!   --set-autostart <off|user|elevated>
//!                             register start with Windows and exit (used by the installer / UAC helper)
//!   --wait-pid <pid>          wait for a previous instance to exit first (restart as administrator)
//!   --portable                keep data next to the executable

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod collector;
#[cfg(windows)]
mod elevation;
#[cfg(windows)]
mod flyout;
#[cfg(windows)]
mod http;
#[cfg(windows)]
mod render;
#[cfg(windows)]
mod settings_ui;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod taskbar;
#[cfg(windows)]
mod theme;
#[cfg(windows)]
mod tiles;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod util;
#[cfg(windows)]
mod widget;

#[cfg(not(windows))]
fn main() {
    eprintln!("Taskbar Monitor is a Windows application.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    std::process::exit(run());
}

#[cfg(windows)]
fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a.eq_ignore_ascii_case(name)).and_then(|i| args.get(i + 1)).map(String::as_str)
}

#[cfg(windows)]
fn run() -> i32 {
    use tm_core::config::{self, AppPaths, Autostart};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| args.iter().any(|a| a.eq_ignore_ascii_case(name));
    let appdata = std::env::var_os("APPDATA").map(std::path::PathBuf::from);
    let paths = AppPaths::resolve(&util::exe_dir(), &args, appdata);
    let _ = std::fs::create_dir_all(&paths.data_dir);
    util::LOG.init(paths.log_file());
    // Release builds abort on panic; leave a trace for bug reports first.
    std::panic::set_hook(Box::new(|info| log!("panic: {info}")));
    let passthrough: Vec<String> = if has("--portable") { vec!["--portable".into()] } else { Vec::new() };

    if let Some(mode) = arg_value(&args, "--set-autostart") {
        let mode = match mode {
            "off" => Autostart::Off,
            "user" => Autostart::User,
            "elevated" => Autostart::Elevated,
            other => {
                log!("unknown autostart mode {other}");
                return 2;
            }
        };
        return match autostart::set(mode) {
            Ok(()) => {
                let path = paths.config_file();
                let (mut settings, _) = config::load(&path);
                settings.general.autostart = mode;
                let _ = config::save(&path, &settings);
                0
            }
            Err(e) => {
                log!("--set-autostart failed: {e}");
                if matches!(e, autostart::AutostartError::NeedsElevation) { 3 } else { 1 }
            }
        };
    }

    if has("--settings") {
        return settings_ui::run(paths, passthrough);
    }

    if let Some(pid) = arg_value(&args, "--wait-pid").and_then(|p| p.parse().ok()) {
        elevation::wait_for_process_exit(pid, 15_000);
    }

    let Some(_guard) = single_instance::acquire() else {
        single_instance::notify_existing();
        return 0;
    };
    app::run(app::Options { paths, passthrough })
}
