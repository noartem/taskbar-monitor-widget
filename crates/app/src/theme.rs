//! System theme: light/dark taskbar, accent color, dark context menus.

use tm_core::color::Rgba;
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::core::{HSTRING, PCSTR};

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

pub fn reg_dword(root: HKEY, key: &str, value: &str) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(key),
            &HSTRING::from(value),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut _ as *mut _),
            Some(&mut size),
        )
        .is_ok()
        .then_some(data)
    }
}

pub fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(key),
            &HSTRING::from(value),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
        .is_ok()
        .then(|| crate::util::from_wide(&buf))
    }
}

pub fn hklm_string(key: &str, value: &str) -> Option<String> {
    reg_string(HKEY_LOCAL_MACHINE, key, value)
}

/// The taskbar follows "Windows mode" (SystemUsesLightTheme), not the app mode.
pub fn taskbar_is_dark() -> bool {
    reg_dword(HKEY_CURRENT_USER, PERSONALIZE, "SystemUsesLightTheme").is_none_or(|v| v == 0)
}

pub fn apps_are_dark() -> bool {
    reg_dword(HKEY_CURRENT_USER, PERSONALIZE, "AppsUseLightTheme").is_some_and(|v| v == 0)
}

pub fn accent_color() -> Option<Rgba> {
    if let Some(abgr) = reg_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\DWM", "AccentColor") {
        return Some(Rgba::from_u8((abgr & 0xFF) as u8, ((abgr >> 8) & 0xFF) as u8, ((abgr >> 16) & 0xFF) as u8));
    }
    let mut argb = 0u32;
    let mut opaque = windows::core::BOOL(0);
    unsafe { DwmGetColorizationColor(&mut argb, &mut opaque).ok()? };
    Some(Rgba::from_u8(((argb >> 16) & 0xFF) as u8, ((argb >> 8) & 0xFF) as u8, (argb & 0xFF) as u8))
}

/// Makes Win32 popup menus follow the given theme (undocumented uxtheme exports used by Explorer and most apps).
pub fn set_menu_theme(dark: bool) {
    unsafe {
        let Ok(uxtheme) = LoadLibraryW(&HSTRING::from("uxtheme.dll")) else { return };
        // #135 SetPreferredAppMode(PreferredAppMode): 2 = ForceDark, 3 = ForceLight.
        if let Some(f) = GetProcAddress(uxtheme, PCSTR(135 as *const u8)) {
            let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(if dark { 2 } else { 3 });
        }
        // #136 FlushMenuThemes().
        if let Some(f) = GetProcAddress(uxtheme, PCSTR(136 as *const u8)) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}
