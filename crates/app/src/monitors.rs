// app::monitors — live display enumeration (Windows)
// §5.5: IDesktopWallpaper-based MonitorEnumerator impl.
// All Windows-specific code is gated with #[cfg(windows)] so this file
// parses on Linux but compiles to nothing after cfg-stripping.

// Cross-platform imports (used by pure functions, available on all platforms).
use wallpaper_shuffler_core::model::{orientation_from_dims, MonitorInfo};

#[cfg(windows)]
use windows::Win32::{
    System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER},
    UI::Shell::{DesktopWallpaper, IDesktopWallpaper},
    UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME,
};

#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplaySettingsW,
    DISPLAY_DEVICEW, DEVMODEW,
    ENUM_CURRENT_SETTINGS, DISPLAY_DEVICE_ACTIVE,
};

#[cfg(windows)]
use wallpaper_shuffler_core::model::{AppError, MonitorEnumerator};

// ---------------------------------------------------------------------------
// Pure layer — no #[cfg(windows)], unit-tested on Linux.
// ---------------------------------------------------------------------------

/// Case-insensitive lookup of `device_path` in the GDI fallback table.
///
/// Returns `Some((width, height))` for the first matching entry, else `None`.
pub(crate) fn match_fallback_dims(
    device_path: &str,
    table: &[(String, u32, u32)],
) -> Option<(u32, u32)> {
    let path_lower = device_path.to_lowercase();
    table
        .iter()
        .find(|(entry_path, _, _)| entry_path.to_lowercase() == path_lower)
        .map(|(_, w, h)| (*w, *h))
}

/// Build a `MonitorInfo` from a device path, optional primary dims, and a
/// GDI fallback table, implementing the 3-step §3.3 policy:
///
/// 1. `primary_dims = Some((w, h))` → available, real dims, derived orientation.
/// 2. Else fallback table matches → available, recovered dims, derived orientation.
/// 3. Else → unavailable (`available: false`, `width: 0`, `height: 0`,
///    `orientation: None`). Still returns `Some(MonitorInfo)` — not skipped.
///
/// Returns `None` only when `device_path` is empty or blank (undecodable id).
pub(crate) fn build_monitor_info(
    device_path: String,
    primary_dims: Option<(u32, u32)>,
    fallback: &[(String, u32, u32)],
) -> Option<MonitorInfo> {
    if device_path.trim().is_empty() {
        return None;
    }

    let (width, height, available) = if let Some((w, h)) = primary_dims {
        (w, h, true)
    } else if let Some((w, h)) = match_fallback_dims(&device_path, fallback) {
        (w, h, true)
    } else {
        (0, 0, false)
    };

    let orientation = if available {
        Some(orientation_from_dims(width, height))
    } else {
        None
    };

    Some(MonitorInfo {
        id: device_path,
        width,
        height,
        orientation,
        available,
    })
}

/// Collect monitors by calling `probe(i)` for each index `0..count`.
/// Probes returning `Err` are **silently skipped** (e.g. undecodable ids);
/// probes returning `Ok(mi)` are collected.
pub(crate) fn collect_monitors<F>(count: u32, mut probe: F) -> Vec<MonitorInfo>
where
    F: FnMut(u32) -> Result<MonitorInfo, String>,
{
    (0..count).filter_map(|i| probe(i).ok()).collect()
}

/// Live monitor enumerator backed by the `IDesktopWallpaper` COM interface.
///
/// Holds an `IDesktopWallpaper` COM pointer obtained via `CoCreateInstance`.
/// The COM apartment must be initialised (via `CoInitializeEx`) before
/// constructing this struct; the caller (app main) is responsible for that.
#[cfg(windows)]
pub struct DesktopWallpaperMonitors {
    idw: IDesktopWallpaper,
}

#[cfg(windows)]
impl DesktopWallpaperMonitors {
    /// Construct by co-creating the `DesktopWallpaper` COM object.
    ///
    /// Caller must have already called `CoInitializeEx` for the current thread.
    pub fn new() -> Result<Self, AppError> {
        // SAFETY: COM is initialised by the caller before this is called.
        let idw: IDesktopWallpaper =
            unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER) }
                .map_err(|e| AppError::Os(format!("CoCreateInstance(DesktopWallpaper): {e}")))?;
        Ok(Self { idw })
    }
}

/// Build the GDI fallback dimension table by walking adapters via
/// `EnumDisplayDevicesW` / `EnumDisplaySettingsW`.
///
/// Each entry is `(device_interface_path, width, height)`. The interface path
/// matches the format returned by `IDesktopWallpaper::GetMonitorDevicePathAt`.
#[cfg(windows)]
fn build_fallback_table() -> Vec<(String, u32, u32)> {
    use std::mem::size_of;

    /// Return a zero-initialised `DISPLAY_DEVICEW` with `cb` set to its own size,
    /// as required by `EnumDisplayDevicesW`.
    fn blank_display_device() -> DISPLAY_DEVICEW {
        DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        }
    }

    let mut table = Vec::new();

    let mut i: u32 = 0;
    loop {
        // Step 1: enumerate adapters (GDI display adapters, e.g. \\.\DISPLAY1).
        let mut adapter = blank_display_device();
        let ok = unsafe { EnumDisplayDevicesW(None, i, &mut adapter, 0) };
        if !ok.as_bool() {
            break;
        }
        i += 1;

        // Only process active adapters.
        if adapter.StateFlags & DISPLAY_DEVICE_ACTIVE == 0 {
            continue;
        }

        // Step 2: get the device-interface path for this adapter's first monitor.
        let mut monitor_dev = blank_display_device();
        let ok = unsafe {
            EnumDisplayDevicesW(
                windows::core::PCWSTR(adapter.DeviceName.as_ptr()),
                0,
                &mut monitor_dev,
                EDD_GET_DEVICE_INTERFACE_NAME,
            )
        };
        if !ok.as_bool() {
            continue;
        }

        // Decode the device-interface path (DeviceID field).
        let interface_path: String = {
            let nul = monitor_dev
                .DeviceID
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(monitor_dev.DeviceID.len());
            match String::from_utf16(&monitor_dev.DeviceID[..nul]) {
                Ok(s) if !s.is_empty() => s,
                _ => continue,
            }
        };

        // Step 3: query current display settings for width/height.
        let mut devmode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let ok = unsafe {
            EnumDisplaySettingsW(
                windows::core::PCWSTR(adapter.DeviceName.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut devmode,
            )
        };
        if !ok.as_bool() {
            continue;
        }

        table.push((interface_path, devmode.dmPelsWidth, devmode.dmPelsHeight));
    }

    table
}

#[cfg(windows)]
impl MonitorEnumerator for DesktopWallpaperMonitors {
    /// Enumerate all active monitors via `IDesktopWallpaper` with GDI fallback.
    ///
    /// Steps:
    /// 1. Build GDI fallback table once.
    /// 2. `GetMonitorDevicePathCount` → total count `n`.
    /// 3. For each index `0..n`: decode id, attempt `GetMonitorRECT` (None on
    ///    failure), call `build_monitor_info` for the 3-step policy.
    ///    Undecodable ids are skipped; recoverable failures produce unavailable
    ///    or fallback-recovered entries.
    fn enumerate(&self) -> Result<Vec<MonitorInfo>, AppError> {
        // Build the GDI fallback table once before the loop.
        let fallback_table = build_fallback_table();

        // Step 1: total monitor count.
        let count: u32 = unsafe { self.idw.GetMonitorDevicePathCount() }
            .map_err(|e| AppError::Os(format!("GetMonitorDevicePathCount: {e}")))?;

        let idw = &self.idw;
        let table = &fallback_table;

        let monitors = collect_monitors(count, |i| {
            // Decode device path — Err skips this index.
            let path_pwstr = unsafe { idw.GetMonitorDevicePathAt(i) }
                .map_err(|e| format!("GetMonitorDevicePathAt({i}): {e}"))?;

            // SAFETY: COM guarantees a valid NUL-terminated wide string.
            let id: String = unsafe { path_pwstr.to_string() }
                .map_err(|e| format!("MonitorId UTF-16 decode at index {i}: {e}"))?;

            // Attempt GetMonitorRECT — convert Err to None (no ? propagation).
            let wide_id: Vec<u16> = id.encode_utf16().chain(std::iter::once(0u16)).collect();
            let primary_dims: Option<(u32, u32)> = unsafe {
                idw.GetMonitorRECT(windows::core::PCWSTR(wide_id.as_ptr()))
            }
            .ok()
            .map(|rect| {
                (
                    (rect.right - rect.left).unsigned_abs(),
                    (rect.bottom - rect.top).unsigned_abs(),
                )
            })
            // A zero-area RECT (power-save / virtual / buggy driver) is treated as
            // unavailable; pass None so the 3-step policy falls through to fallback
            // or marks the monitor unavailable rather than setting available=true
            // with width=0 / height=0.
            .filter(|&(w, h)| w > 0 && h > 0);

            // 3-step policy: primary → fallback → unavailable.
            build_monitor_info(id.clone(), primary_dims, table)
                .ok_or_else(|| format!("undecodable id at index {i}"))
        });

        Ok(monitors)
    }
}

#[cfg(test)]
mod tests {
    use super::{build_monitor_info, match_fallback_dims};
    use wallpaper_shuffler_core::model::{MonitorId, MonitorInfo, Orientation};

    fn sample() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::from("\\\\?\\DISPLAY#TEST#0001"),
            width: 1920,
            height: 1080,
            orientation: Some(Orientation::Landscape),
            available: true,
        }
    }

    #[test]
    fn sample_monitor_is_available_and_landscape() {
        let m = sample();
        assert!(m.available);
        assert_eq!(m.orientation, Some(Orientation::Landscape));
        assert_eq!(m.width, 1920);
        assert_eq!(m.height, 1080);
    }

    // --- match_fallback_dims ---

    #[test]
    fn match_fallback_dims_exact_match() {
        let table = vec![(r"\\?\DISPLAY#ACR0763#5&abc&0&UID0#{GUID}".to_string(), 2560, 1440)];
        let result = match_fallback_dims(r"\\?\DISPLAY#ACR0763#5&abc&0&UID0#{GUID}", &table);
        assert_eq!(result, Some((2560, 1440)));
    }

    #[test]
    fn match_fallback_dims_case_insensitive() {
        let table = vec![(r"\\?\DISPLAY#ACR0763#5&ABC&0&UID0#{GUID}".to_string(), 1920, 1200)];
        let result = match_fallback_dims(r"\\?\display#acr0763#5&abc&0&uid0#{guid}", &table);
        assert_eq!(result, Some((1920, 1200)));
    }

    #[test]
    fn match_fallback_dims_no_match() {
        let table = vec![(r"\\?\DISPLAY#OTHER#1&foo&0&UID0#{GUID}".to_string(), 1920, 1080)];
        let result = match_fallback_dims(r"\\?\DISPLAY#ACR0763#5&abc&0&UID0#{GUID}", &table);
        assert_eq!(result, None);
    }

    #[test]
    fn match_fallback_dims_empty_table() {
        let table: Vec<(String, u32, u32)> = vec![];
        let result = match_fallback_dims(r"\\?\DISPLAY#ACR0763#5&abc&0&UID0#{GUID}", &table);
        assert_eq!(result, None);
    }

    // --- build_monitor_info ---

    #[test]
    fn build_monitor_info_primary_dims_available() {
        let mi = build_monitor_info(
            r"\\?\DISPLAY#TEST#0001".to_string(),
            Some((1920, 1080)),
            &[],
        );
        let mi = mi.expect("should be Some");
        assert!(mi.available);
        assert_eq!(mi.orientation, Some(Orientation::Landscape));
        assert_eq!(mi.width, 1920);
        assert_eq!(mi.height, 1080);
    }

    #[test]
    fn build_monitor_info_fallback_recovered_acer_case() {
        let fallback = vec![(r"\\?\DISPLAY#ACR0763#5&abc&0&UID0#{GUID}".to_string(), 2560, 1440)];
        let mi = build_monitor_info(
            r"\\?\DISPLAY#ACR0763#5&abc&0&UID0#{GUID}".to_string(),
            None,
            &fallback,
        );
        let mi = mi.expect("should be Some via fallback");
        assert!(mi.available);
        assert_eq!(mi.orientation, Some(Orientation::Landscape));
        assert_eq!(mi.width, 2560);
        assert_eq!(mi.height, 1440);
    }

    #[test]
    fn build_monitor_info_unavailable_when_no_primary_and_no_fallback() {
        let mi = build_monitor_info(
            r"\\?\DISPLAY#UNKNOWN#1&foo&0&UID0#{GUID}".to_string(),
            None,
            &[],
        );
        let mi = mi.expect("should still be Some (not skipped)");
        assert!(!mi.available);
        assert_eq!(mi.orientation, None);
        assert_eq!(mi.width, 0);
        assert_eq!(mi.height, 0);
    }

    #[test]
    fn build_monitor_info_empty_device_path_returns_none() {
        assert!(build_monitor_info("".to_string(), None, &[]).is_none());
    }

    #[test]
    fn build_monitor_info_blank_device_path_returns_none() {
        assert!(build_monitor_info("   ".to_string(), None, &[]).is_none());
    }
}
