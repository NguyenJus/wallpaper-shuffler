// app::autostart — HKCU Run registry (Windows)
//
// Writes/deletes/checks the value
//   HKCU\Software\Microsoft\Windows\CurrentVersion\Run  "wallpaper-shuffler"
// so the app launches at login.
//
// The entire implementation is behind #[cfg(windows)] so this file parses
// (syntax-checks) on Linux without pulling in windows-crate types.

#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    },
};

#[cfg(windows)]
use wallpaper_shuffler_core::model::{AppError, AutostartManager};

/// Registry-key path (no trailing NUL; widened at call-sites with `to_wide`).
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Value name written under the Run key.
#[cfg(windows)]
const VALUE_NAME: &str = "wallpaper-shuffler";

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Encode a Rust `&str` as a NUL-terminated UTF-16 Vec.
#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0u16)).collect()
}

/// Open the Run key with the requested access rights.
/// Returns the raw `HKEY`; caller must `RegCloseKey` it.
#[cfg(windows)]
fn open_run_key(access: u32) -> Result<HKEY, AppError> {
    use windows::Win32::System::Registry::REG_SAM_FLAGS;

    let subkey = to_wide(RUN_KEY);
    let mut hkey = HKEY::default();
    // SAFETY: subkey is NUL-terminated UTF-16; hkey is written-to output.
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            REG_SAM_FLAGS(access),
            &mut hkey,
        )
    };
    if rc.is_ok() {
        Ok(hkey)
    } else {
        Err(AppError::Os(format!("RegOpenKeyExW failed: {:?}", rc)))
    }
}

// ---------------------------------------------------------------------------
// Public struct
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub struct RunKeyAutostart;

#[cfg(windows)]
impl AutostartManager for RunKeyAutostart {
    /// Returns `true` when the value is present (regardless of its data).
    fn is_enabled(&self) -> Result<bool, AppError> {
        let value_name = to_wide(VALUE_NAME);

        let hkey = open_run_key(KEY_QUERY_VALUE.0)?;

        // SAFETY: hkey is valid; all pointer args are correctly sized/aligned.
        let rc =
            unsafe { RegQueryValueExW(hkey, PCWSTR(value_name.as_ptr()), None, None, None, None) };
        // SAFETY: hkey is valid and we are done with it.
        unsafe { RegCloseKey(hkey) };

        // ERROR_SUCCESS (0) → present; ERROR_FILE_NOT_FOUND (2) → absent.
        match rc.0 {
            0 => Ok(true),
            2 => Ok(false), // ERROR_FILE_NOT_FOUND
            _ => Err(AppError::Os(format!("RegQueryValueExW failed: {:?}", rc))),
        }
    }

    fn set_enabled(&self, on: bool) -> Result<(), AppError> {
        if on {
            self.write_value()
        } else {
            self.delete_value()
        }
    }
}

#[cfg(windows)]
impl RunKeyAutostart {
    /// Write the quoted exe path as a REG_SZ under the Run key.
    fn write_value(&self) -> Result<(), AppError> {
        let exe = std::env::current_exe().map_err(AppError::Io)?;
        // Quoted so paths with spaces survive shell re-invocation.
        let data_str = format!("\"{}\"", exe.display());
        let data_wide = to_wide(&data_str);
        // Byte length of the NUL-terminated UTF-16 string.
        let byte_len = (data_wide.len() * 2) as u32;

        let value_name = to_wide(VALUE_NAME);
        let hkey = open_run_key(KEY_SET_VALUE.0)?;

        // SAFETY: hkey valid; data slice covers `byte_len` bytes.
        let rc = unsafe {
            RegSetValueExW(
                hkey,
                PCWSTR(value_name.as_ptr()),
                0,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    data_wide.as_ptr() as *const u8,
                    byte_len as usize,
                )),
            )
        };
        // SAFETY: hkey valid, closing.
        unsafe { RegCloseKey(hkey) };

        if rc.is_ok() {
            Ok(())
        } else {
            Err(AppError::Os(format!("RegSetValueExW failed: {:?}", rc)))
        }
    }

    /// Remove the value from the Run key (idempotent — absent value is OK).
    fn delete_value(&self) -> Result<(), AppError> {
        let value_name = to_wide(VALUE_NAME);

        let hkey = match open_run_key(KEY_SET_VALUE.0) {
            Ok(k) => k,
            Err(e) => return Err(e),
        };

        // SAFETY: hkey valid; value_name is NUL-terminated UTF-16.
        let rc = unsafe { RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr())) };
        // SAFETY: hkey valid, closing.
        unsafe { RegCloseKey(hkey) };

        match rc.0 {
            0 => Ok(()), // deleted
            2 => Ok(()), // ERROR_FILE_NOT_FOUND — already absent
            _ => Err(AppError::Os(format!("RegDeleteValueW failed: {:?}", rc))),
        }
    }
}
