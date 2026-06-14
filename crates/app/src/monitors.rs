// app::monitors — live display enumeration (Windows)
// §5.5: IDesktopWallpaper-based MonitorEnumerator impl.
// All Windows-specific code is gated with #[cfg(windows)] so this file
// parses on Linux but compiles to nothing after cfg-stripping.

#[cfg(windows)]
use windows::Win32::{
    Foundation::RECT,
    System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER},
    UI::Shell::{DesktopWallpaper, IDesktopWallpaper},
};

#[cfg(windows)]
use wallpaper_shuffler_core::model::{
    orientation_from_dims, AppError, MonitorEnumerator, MonitorInfo,
};

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

#[cfg(windows)]
impl MonitorEnumerator for DesktopWallpaperMonitors {
    /// Enumerate all active monitors via `IDesktopWallpaper`.
    ///
    /// Steps:
    /// 1. `GetMonitorDevicePathCount` → total count `n`.
    /// 2. For each index `0..n`: `GetMonitorDevicePathAt(i)` → stable device
    ///    path string used as `MonitorId`.
    /// 3. `GetMonitorRECT(path)` → `RECT`; derive width/height → `Orientation`.
    fn enumerate(&self) -> Result<Vec<MonitorInfo>, AppError> {
        // Step 1: total monitor count.
        let count: u32 = unsafe { self.idw.GetMonitorDevicePathCount() }
            .map_err(|e| AppError::Os(format!("GetMonitorDevicePathCount: {e}")))?;

        let mut monitors = Vec::with_capacity(count as usize);

        for i in 0..count {
            // Step 2: stable device path (MonitorId).
            let path_pwstr = unsafe { self.idw.GetMonitorDevicePathAt(i) }
                .map_err(|e| AppError::Os(format!("GetMonitorDevicePathAt({i}): {e}")))?;

            // Convert the PWSTR (wide string) to a Rust String.
            // SAFETY: the COM implementation guarantees a valid NUL-terminated
            // wide string for the lifetime of the call.
            let id: String = unsafe { path_pwstr.to_string() }
                .map_err(|e| AppError::Os(format!("MonitorId UTF-16 decode at index {i}: {e}")))?;

            // Step 3: RECT → width / height → Orientation.
            // GetMonitorRECT takes a PCWSTR; re-encode the Rust string as wide.
            let wide_id: Vec<u16> = id.encode_utf16().chain(std::iter::once(0u16)).collect();
            let rect: RECT = unsafe {
                self.idw
                    .GetMonitorRECT(windows::core::PCWSTR(wide_id.as_ptr()))
            }
            .map_err(|e| AppError::Os(format!("GetMonitorRECT({id}): {e}")))?;

            // RECT fields are i32 (left/top/right/bottom).
            let width = (rect.right - rect.left).unsigned_abs();
            let height = (rect.bottom - rect.top).unsigned_abs();
            let orientation = orientation_from_dims(width, height);

            monitors.push(MonitorInfo {
                id,
                width,
                height,
                orientation,
            });
        }

        Ok(monitors)
    }
}
