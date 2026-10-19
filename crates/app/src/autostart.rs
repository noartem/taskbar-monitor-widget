//! Start with Windows: HKCU Run key (normal rights) or a Task Scheduler task (administrator, no UAC prompt).

use std::os::windows::process::CommandExt;
use std::process::Command;

use tm_core::config::Autostart;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::core::HSTRING;

use crate::{elevation, util};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "TaskbarMonitor";
pub const TASK_NAME: &str = "TaskbarMonitor";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug)]
pub enum AutostartError {
    /// Creating or removing the elevated task requires administrator rights.
    NeedsElevation,
    Failed(String),
}

impl std::fmt::Display for AutostartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutostartError::NeedsElevation => write!(f, "administrator rights required"),
            AutostartError::Failed(e) => write!(f, "{e}"),
        }
    }
}

fn command_line() -> String {
    format!("\"{}\" --autostart", util::exe_path().display())
}

/// What is currently registered in Windows (not what the config says).
pub fn current() -> Autostart {
    if task_exists() {
        Autostart::Elevated
    } else if run_value().is_some() {
        Autostart::User
    } else {
        Autostart::Off
    }
}

pub fn set(mode: Autostart) -> Result<(), AutostartError> {
    let has_task = task_exists();
    let needs_admin = matches!(mode, Autostart::Elevated) || has_task;
    if needs_admin && !elevation::is_elevated() {
        return Err(AutostartError::NeedsElevation);
    }
    match mode {
        Autostart::Off => {
            if has_task {
                delete_task()?;
            }
            delete_run_value();
        }
        Autostart::User => {
            if has_task {
                delete_task()?;
            }
            set_run_value(&command_line())?;
        }
        Autostart::Elevated => {
            create_task()?;
            delete_run_value();
        }
    }
    Ok(())
}

/// Applies `mode`, asking for administrator rights through UAC when needed.
/// Returns `Ok(false)` if the user cancelled the UAC prompt.
pub fn set_with_elevation(mode: Autostart) -> Result<bool, AutostartError> {
    match set(mode) {
        Ok(()) => Ok(true),
        Err(AutostartError::NeedsElevation) => {
            let arg = match mode {
                Autostart::Off => "off",
                Autostart::User => "user",
                Autostart::Elevated => "elevated",
            };
            match elevation::run_self_elevated(&["--set-autostart", arg], true) {
                Ok(elevation::ElevatedRun::Cancelled) => Ok(false),
                Ok(elevation::ElevatedRun::Finished(0)) | Ok(elevation::ElevatedRun::Started) => Ok(true),
                Ok(elevation::ElevatedRun::Finished(code)) => Err(AutostartError::Failed(format!("exit code {code}"))),
                Err(e) => Err(AutostartError::Failed(e.message())),
            }
        }
        Err(e) => Err(e),
    }
}

fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), Some(0), access, &mut key).is_ok().then_some(key) }
}

fn run_value() -> Option<String> {
    let key = open_run_key(KEY_QUERY_VALUE)?;
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegQueryValueExW(key, &HSTRING::from(VALUE_NAME), None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut size)).is_ok()
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    ok.then(|| util::from_wide(&buf))
}

fn set_run_value(value: &str) -> Result<(), AutostartError> {
    let key = open_run_key(KEY_SET_VALUE).ok_or_else(|| AutostartError::Failed("cannot open Run key".into()))?;
    let data = util::wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
    let r = unsafe { RegSetValueExW(key, &HSTRING::from(VALUE_NAME), Some(0), REG_SZ, Some(bytes)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    r.ok().map_err(|e| AutostartError::Failed(e.message()))
}

fn delete_run_value() {
    if let Some(key) = open_run_key(KEY_SET_VALUE) {
        unsafe {
            let _ = RegDeleteValueW(key, &HSTRING::from(VALUE_NAME));
            let _ = RegCloseKey(key);
        }
    }
}

fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("schtasks.exe").args(args).creation_flags(CREATE_NO_WINDOW).output()
}

pub fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", TASK_NAME])
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(TASK_NAME))
        .unwrap_or(false)
}

fn delete_task() -> Result<(), AutostartError> {
    let out = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]).map_err(|e| AutostartError::Failed(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(AutostartError::Failed(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn create_task() -> Result<(), AutostartError> {
    let exe = util::exe_path();
    let dir = util::exe_dir();
    let user = xml_escape(&current_user());
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts Taskbar Monitor with administrator rights at logon.</Description>
    <URI>\{TASK_NAME}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>PT3S</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>--autostart</Arguments>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        xml_escape(&exe.display().to_string()),
        xml_escape(&dir.display().to_string()),
    );

    let path = std::env::temp_dir().join(format!("taskbar-monitor-task-{}.xml", std::process::id()));
    // Task Scheduler expects UTF-16 LE with BOM when the declaration says UTF-16.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&path, bytes).map_err(|e| AutostartError::Failed(e.to_string()))?;
    let path_str = path.display().to_string();
    let out = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &path_str, "/F"]);
    let _ = std::fs::remove_file(&path);
    let out = out.map_err(|e| AutostartError::Failed(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(AutostartError::Failed(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}
