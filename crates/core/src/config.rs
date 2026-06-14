// core::config — data model + TOML load/save

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::{AppError, CycleMode, FitMode, MonitorId};

pub const MIN_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_INTERVAL_SECS: u64 = 1800;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub interval_secs: u64,
    pub cycle_mode: CycleMode,
    pub fit_mode: FitMode,
    pub autostart: bool,
    /// Per-monitor folder assignments, keyed by stable MonitorId.
    pub monitors: BTreeMap<MonitorId, MonitorConfig>,
}

#[derive(Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// One or more folders assigned to this monitor. Empty => unconfigured.
    pub folders: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            interval_secs: DEFAULT_INTERVAL_SECS,
            cycle_mode: CycleMode::Sequential,
            fit_mode: FitMode::Fill,
            autostart: true,
            monitors: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Clamp interval to >= MIN_INTERVAL_SECS. Call after load and after edits.
    pub fn clamped(mut self) -> Config {
        if self.interval_secs < MIN_INTERVAL_SECS {
            self.interval_secs = MIN_INTERVAL_SECS;
        }
        self
    }

    /// Parse TOML text. Errors => caller writes defaults + opens settings.
    pub fn from_toml(text: &str) -> Result<Config, AppError> {
        toml::from_str(text).map_err(|e| AppError::Config(e.to_string()))
    }

    pub fn to_toml(&self) -> Result<String, AppError> {
        toml::to_string(self).map_err(|e| AppError::Config(e.to_string()))
    }
}

/// Load from disk; on missing OR parse-error, return (Config::default(), true)
/// where the bool = "was reset/needs settings". On success => (cfg, false).
pub fn load_or_default(path: &Path) -> (Config, bool) {
    match std::fs::read_to_string(path) {
        Ok(text) => match Config::from_toml(&text) {
            Ok(cfg) => (cfg, false),
            Err(_) => (Config::default(), true),
        },
        Err(_) => (Config::default(), true),
    }
}

/// Atomic-ish save: write to temp then rename. Creates parent dir.
pub fn save(path: &Path, cfg: &Config) -> Result<(), AppError> {
    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }

    let text = cfg.to_toml()?;

    // Write to a temp file in the same directory, then rename (atomic-ish)
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, &text).map_err(AppError::Io)?;
    std::fs::rename(&tmp_path, path).map_err(AppError::Io)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ---- default matches §defaults ----

    #[test]
    fn default_matches_spec() {
        let cfg = Config::default();
        assert_eq!(cfg.interval_secs, DEFAULT_INTERVAL_SECS); // 1800
        assert_eq!(cfg.cycle_mode, CycleMode::Sequential);
        assert_eq!(cfg.fit_mode, FitMode::Fill);
        assert!(cfg.autostart);
        assert!(cfg.monitors.is_empty());
    }

    // ---- to_toml <-> from_toml round-trip ----

    #[test]
    fn round_trip_preserves_populated_config() {
        let mut monitors = BTreeMap::new();
        monitors.insert(
            "monitor-1".to_string(),
            MonitorConfig {
                folders: vec![PathBuf::from("C:\\Wallpapers\\Landscape")],
            },
        );
        monitors.insert(
            "monitor-2".to_string(),
            MonitorConfig {
                folders: vec![
                    PathBuf::from("C:\\Wallpapers\\Portrait"),
                    PathBuf::from("C:\\Wallpapers\\Portrait2"),
                ],
            },
        );

        let original = Config {
            interval_secs: 300,
            cycle_mode: CycleMode::Shuffle,
            fit_mode: FitMode::Fit,
            autostart: false,
            monitors,
        };

        let toml_text = original.to_toml().expect("to_toml should succeed");
        let restored = Config::from_toml(&toml_text).expect("from_toml should succeed");

        assert_eq!(original, restored);
    }

    // ---- from_toml garbage => Err ----

    #[test]
    fn from_toml_garbage_returns_err() {
        let result = Config::from_toml("this is not valid toml ][[[");
        assert!(result.is_err(), "garbage input should return Err");
    }

    #[test]
    fn from_toml_wrong_type_returns_err() {
        // interval_secs expects u64, not a string
        let result = Config::from_toml(r#"interval_secs = "not-a-number""#);
        assert!(result.is_err(), "wrong type should return Err");
    }

    // ---- load_or_default on garbage file => (default, true) ----

    #[test]
    fn load_or_default_garbage_file_returns_default_and_true() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad_config.toml");
        std::fs::write(&path, b"garbage content ][[[").expect("write");

        let (cfg, needs_settings) = load_or_default(&path);
        assert!(needs_settings, "garbage file should signal needs_settings");
        assert_eq!(cfg, Config::default());
    }

    // ---- load_or_default on missing file => (default, true) ----

    #[test]
    fn load_or_default_missing_file_returns_default_and_true() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent_config.toml");

        let (cfg, needs_settings) = load_or_default(&path);
        assert!(needs_settings, "missing file should signal needs_settings");
        assert_eq!(cfg, Config::default());
    }

    // ---- clamped() raises sub-minimum to 60, leaves valid ----

    #[test]
    fn clamped_raises_sub_minimum_to_60() {
        let cfg = Config {
            interval_secs: 0,
            ..Config::default()
        };
        assert_eq!(cfg.clamped().interval_secs, MIN_INTERVAL_SECS);
    }

    #[test]
    fn clamped_raises_1_to_60() {
        let cfg = Config {
            interval_secs: 1,
            ..Config::default()
        };
        assert_eq!(cfg.clamped().interval_secs, MIN_INTERVAL_SECS);
    }

    #[test]
    fn clamped_raises_59_to_60() {
        let cfg = Config {
            interval_secs: 59,
            ..Config::default()
        };
        assert_eq!(cfg.clamped().interval_secs, 60);
    }

    #[test]
    fn clamped_leaves_exactly_60() {
        let cfg = Config {
            interval_secs: 60,
            ..Config::default()
        };
        assert_eq!(cfg.clamped().interval_secs, 60);
    }

    #[test]
    fn clamped_leaves_valid_above_minimum() {
        let cfg = Config {
            interval_secs: 3600,
            ..Config::default()
        };
        assert_eq!(cfg.clamped().interval_secs, 3600);
    }

    #[test]
    fn clamped_leaves_default_interval_unchanged() {
        let cfg = Config::default();
        assert_eq!(cfg.clamped().interval_secs, DEFAULT_INTERVAL_SECS);
    }

    // ---- serde(default) tolerates missing fields ----

    #[test]
    fn from_toml_empty_string_uses_defaults() {
        let cfg = Config::from_toml("").expect("empty TOML should use defaults");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn from_toml_partial_config_fills_missing_fields_from_defaults() {
        let text = r#"interval_secs = 120"#;
        let cfg = Config::from_toml(text).expect("partial config should succeed");
        assert_eq!(cfg.interval_secs, 120);
        // All other fields should be defaults
        assert_eq!(cfg.cycle_mode, CycleMode::Sequential);
        assert_eq!(cfg.fit_mode, FitMode::Fill);
        assert!(cfg.autostart);
        assert!(cfg.monitors.is_empty());
    }

    #[test]
    fn from_toml_unknown_fields_tolerated() {
        // serde(default) on struct doesn't automatically deny unknown fields;
        // unknown fields should not cause a parse error with toml
        let text = r#"
interval_secs = 300
unknown_future_field = "some value"
"#;
        // toml::from_str by default ignores unknown fields
        let result = Config::from_toml(text);
        assert!(result.is_ok(), "unknown fields should be tolerated");
        let cfg = result.unwrap();
        assert_eq!(cfg.interval_secs, 300);
    }

    // ---- save then load_or_default round-trips via real tempfile ----

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("subdir").join("config.toml");

        let mut monitors = BTreeMap::new();
        monitors.insert(
            "\\\\?\\DISPLAY#TEST#1".to_string(),
            MonitorConfig {
                folders: vec![PathBuf::from("/tmp/wallpapers")],
            },
        );

        let original = Config {
            interval_secs: 600,
            cycle_mode: CycleMode::PureRandom,
            fit_mode: FitMode::Stretch,
            autostart: false,
            monitors,
        };

        save(&path, &original).expect("save should succeed");
        assert!(path.exists(), "config file should exist after save");

        let (restored, needs_settings) = load_or_default(&path);
        assert!(
            !needs_settings,
            "successfully saved config should not need settings"
        );
        assert_eq!(original, restored);
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("deep").join("config.toml");

        let cfg = Config::default();
        save(&path, &cfg).expect("save should create parent dirs and succeed");
        assert!(path.exists(), "config file should exist after save");
    }

    #[test]
    fn load_or_default_success_returns_false_for_needs_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        let cfg = Config::default();
        save(&path, &cfg).expect("save");

        let (_, needs_settings) = load_or_default(&path);
        assert!(
            !needs_settings,
            "valid config should return needs_settings=false"
        );
    }
}
