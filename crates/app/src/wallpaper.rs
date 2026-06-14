// app::wallpaper — set one image on one monitor (Windows)
//
// All Windows-specific items are behind #[cfg(windows)] so this file parses
// on Linux (syntax-check only) and compiles to nothing after cfg-stripping.

#[cfg(windows)]
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER},
    Win32::UI::Shell::{
        DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL,
        DWPOS_FIT, DWPOS_STRETCH, DWPOS_TILE,
    },
};

#[cfg(windows)]
use wallpaper_shuffler_core::model::{AppError, FitMode, MonitorId, WallpaperSetter};

/// Windows implementation of [`WallpaperSetter`] backed by
/// [`IDesktopWallpaper`].
///
/// COM must already be initialised on the calling thread
/// (`CoInitializeEx` / `CoInitialize`) before constructing this type.
#[cfg(windows)]
pub struct DesktopWallpaperSetter {
    inner: IDesktopWallpaper,
}

#[cfg(windows)]
impl DesktopWallpaperSetter {
    /// Create a new instance by CoCreating `DesktopWallpaper`.
    ///
    /// Returns [`AppError::Os`] on any COM failure.
    pub fn new() -> Result<Self, AppError> {
        // SAFETY: COM must be initialised before this is called.
        let inner: IDesktopWallpaper =
            unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER) }
                .map_err(|e| AppError::Os(format!("CoCreateInstance(DesktopWallpaper): {e}")))?;
        Ok(Self { inner })
    }
}

/// Map our `FitMode` to the Win32 `DESKTOP_WALLPAPER_POSITION` enum.
#[cfg(windows)]
fn fit_to_position(fit: FitMode) -> DESKTOP_WALLPAPER_POSITION {
    match fit {
        FitMode::Fill => DWPOS_FILL,
        FitMode::Fit => DWPOS_FIT,
        FitMode::Stretch => DWPOS_STRETCH,
        FitMode::Center => DWPOS_CENTER,
        FitMode::Tile => DWPOS_TILE,
    }
}

/// Decide whether a Windows `HRESULT`/`io::Error` represents a missing or
/// unsupported file (→ `AppError::BadImage`) vs. a general OS error
/// (→ `AppError::Os`).
#[cfg(windows)]
fn classify_set_error(path: &std::path::Path, err: windows::core::Error) -> AppError {
    // ERROR_FILE_NOT_FOUND (0x80070002) or ERROR_PATH_NOT_FOUND (0x80070003)
    // indicate the file does not exist.
    // E_INVALIDARG (0x80070057) / HRESULT 0x80004005 (E_FAIL) are produced by
    // Windows Shell when the image format is not supported.
    //
    // We map these to BadImage so the scheduler can call skip_and_next.
    // Everything else becomes Os.
    let code = err.code().0 as u32;
    let bad_image = matches!(
        code,
        0x8007_0002  // ERROR_FILE_NOT_FOUND
        | 0x8007_0003  // ERROR_PATH_NOT_FOUND
        | 0x8007_0005  // ERROR_ACCESS_DENIED (treat as unreadable → bad image)
        | 0x8007_000d  // ERROR_INVALID_DATA
        | 0x8007_007b  // ERROR_INVALID_NAME
        | 0x8007_0057 // E_INVALIDARG (bad format / unsupported)
    );
    let msg = format!("{}: {err}", path.display());
    if bad_image {
        AppError::BadImage(msg)
    } else {
        AppError::Os(msg)
    }
}

#[cfg(windows)]
impl WallpaperSetter for DesktopWallpaperSetter {
    fn set(
        &self,
        monitor: &MonitorId,
        image: &std::path::Path,
        fit: FitMode,
    ) -> Result<(), AppError> {
        // 1. Map fit mode and apply globally.
        //    SetPosition is global to the desktop; setting it before each
        //    SetWallpaper is acceptable and cheap.
        let position = fit_to_position(fit);
        unsafe { self.inner.SetPosition(position) }
            .map_err(|e| AppError::Os(format!("IDesktopWallpaper::SetPosition: {e}")))?;

        // 2. Convert the path to a wide string.
        let path_wide = HSTRING::from(image.as_os_str());

        // 3. Convert the monitor id to PCWSTR.
        //    The monitor id is the device-path string obtained from
        //    IDesktopWallpaper::GetMonitorDevicePathAt; we need to round-trip
        //    it back to a wide string pointer for SetWallpaper.
        let monitor_wide = HSTRING::from(monitor.as_str());
        let monitor_pcwstr = PCWSTR(monitor_wide.as_ptr());

        // 4. Apply the wallpaper to this monitor.
        unsafe { self.inner.SetWallpaper(monitor_pcwstr, &path_wide) }
            .map_err(|e| classify_set_error(image, e))?;

        Ok(())
    }
}
