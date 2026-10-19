//! Value formatting shared by the widget, flyout, tooltip and tray icon.

use crate::config::NetworkUnit;

const BYTE_UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
const BIT_UNITS: [&str; 5] = ["b", "Kb", "Mb", "Gb", "Tb"];

fn decimals_for(value: f64, unit_index: usize, decimals: i32) -> usize {
    if decimals >= 0 {
        decimals as usize
    } else if unit_index == 0 || value.abs() >= 100.0 {
        0
    } else {
        1
    }
}

pub fn percent(value: f64, decimals: i32) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    format!("{:.*}%", decimals.max(0) as usize, value)
}

pub fn temperature(value: f64, decimals: i32) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    format!("{:.*}°C", decimals.max(0) as usize, value)
}

/// 1536 → "1.5 KB" (binary steps, like Task Manager). Values stay below 1000 to keep widths stable.
pub fn bytes(mut value: f64, decimals: i32) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    let mut unit = 0;
    while value.abs() >= 1000.0 && unit < BYTE_UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{:.*} {}", decimals_for(value, unit, decimals), value, BYTE_UNITS[unit])
}

pub fn rate(bytes_per_sec: f64, unit: NetworkUnit, decimals: i32) -> String {
    if !bytes_per_sec.is_finite() {
        return "—".into();
    }
    match unit {
        NetworkUnit::Bytes => format!("{}/s", bytes(bytes_per_sec, decimals)),
        NetworkUnit::Bits => {
            let mut v = bytes_per_sec * 8.0;
            let mut u = 0;
            while v.abs() >= 1000.0 && u < BIT_UNITS.len() - 1 {
                v /= 1000.0;
                u += 1;
            }
            format!("{:.*} {}ps", decimals_for(v, u, decimals), v, BIT_UNITS[u])
        }
    }
}

pub fn generic(value: f64, unit: &str, decimals: i32) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    let d = if decimals >= 0 {
        decimals as usize
    } else if value.abs() >= 100.0 {
        0
    } else {
        1
    };
    match unit {
        "" => format!("{value:.d$}"),
        "%" | "°C" | "°F" => format!("{value:.d$}{unit}"),
        _ => format!("{value:.d$} {unit}"),
    }
}

/// "3d 4h", "2h 05m", "12m".
pub fn duration(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{}m", if m == 0 && secs > 0 { 1 } else { m })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(percent(42.4, -1), "42%");
        assert_eq!(percent(42.45, 1), "42.5%");
        assert_eq!(bytes(512.0, -1), "512 B");
        assert_eq!(bytes(1536.0, -1), "1.5 KB");
        assert_eq!(bytes(1000.0, -1), "1.0 KB");
        assert_eq!(bytes(250.0 * 1024.0 * 1024.0, -1), "250 MB");
        assert_eq!(rate(1_250_000.0, NetworkUnit::Bits, -1), "10.0 Mbps");
        assert_eq!(rate(2048.0, NetworkUnit::Bytes, -1), "2.0 KB/s");
        assert_eq!(temperature(54.6, -1), "55°C");
        assert_eq!(generic(1234.5, "RPM", -1), "1234 RPM");
        assert_eq!(generic(1.23456, "V", 2), "1.23 V");
        assert_eq!(duration(3 * 86_400 + 4 * 3600 + 5), "3d 4h");
        assert_eq!(duration(2 * 3600 + 5 * 60), "2h 05m");
        assert_eq!(duration(30), "1m");
        assert_eq!(duration(0), "0m");
        assert_eq!(percent(f64::NAN, -1), "—");
    }
}
