/// Stable monitor identity = the Windows device-path string.
/// Survives reboot/replug, so config keys to it.
pub type MonitorId = String;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Orientation {
    Landscape,
    Portrait,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum CycleMode {
    Sequential,
    Shuffle,
    PureRandom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum FitMode {
    Fill,
    Fit,
    Stretch,
    Center,
    Tile,
}

/// A live monitor as enumerated from the OS.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub width: u32,                       // 0 when unknown
    pub height: u32,                      // 0 when unknown
    pub orientation: Option<Orientation>, // None when unavailable / dims unknown
    pub available: bool,                  // false => skip wallpaper application
}

/// Derive orientation from physical dimensions.
/// Portrait when height > width, else Landscape.
pub fn orientation_from_dims(width: u32, height: u32) -> Orientation {
    if height > width {
        Orientation::Portrait
    } else {
        Orientation::Landscape
    }
}

/// SEAM: enumerate monitors. Real impl uses IDesktopWallpaper; tests use a fake.
pub trait MonitorEnumerator {
    fn enumerate(&self) -> Result<Vec<MonitorInfo>, AppError>;
}

/// SEAM: apply a wallpaper image to one monitor with a fit mode.
pub trait WallpaperSetter {
    fn set(
        &self,
        monitor: &MonitorId,
        image: &std::path::Path,
        fit: FitMode,
    ) -> Result<(), AppError>;
}

/// SEAM: manage the HKCU Run autostart entry.
pub trait AutostartManager {
    fn is_enabled(&self) -> Result<bool, AppError>;
    fn set_enabled(&self, on: bool) -> Result<(), AppError>;
}

#[derive(Debug)]
pub enum AppError {
    Io(std::io::Error),
    Config(String),
    Os(String),
    BadImage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_landscape_when_wider() {
        assert_eq!(orientation_from_dims(1920, 1080), Orientation::Landscape);
    }

    #[test]
    fn orientation_portrait_when_taller() {
        assert_eq!(orientation_from_dims(1080, 1920), Orientation::Portrait);
    }

    #[test]
    fn orientation_landscape_when_square() {
        // equal width and height is not portrait (height > width is false)
        assert_eq!(orientation_from_dims(1000, 1000), Orientation::Landscape);
    }
}
