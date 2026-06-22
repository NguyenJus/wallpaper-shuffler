// app::main — wiring + Win32 message loop (§5.10, §7)
//
// On Linux this whole crate compiles to (almost) nothing: every Windows item
// is gated behind `#[cfg(windows)]`, so after cfg-stripping only the
// `#[cfg(not(windows))]` stub remains. The sibling modules
// (monitors / wallpaper / autostart / tray / settings_ui) are plain `mod`
// declarations whose Windows-only contents are likewise cfg-stripped, so the
// Linux build only parses them.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod autostart;
mod monitors;
mod settings_ui;
mod tray;
mod wallpaper;

// ---------------------------------------------------------------------------
// Shared app-controller seam (§5.8 / §5.9 dispatch target).
//
// `AppState` is the single mutable hub that the tray and the settings window
// dispatch into, avoiding circular deps: tray/settings_ui hold an
// `Rc<RefCell<AppState>>` and call methods on it. The Win32 timer (owned by
// `main`) drives `tick`. Only `main` knows about HWND / timer ids.
//
// The concrete scheduler type is pinned here to the real seam impls.
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod app_state {
    use std::path::PathBuf;
    use std::rc::Rc;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use wallpaper_shuffler_core::config::{self, Config};
    use wallpaper_shuffler_core::model::{
        AutostartManager, CycleMode, FitMode, MonitorEnumerator, MonitorId, MonitorInfo,
    };
    use wallpaper_shuffler_core::scheduler::{Scheduler, TickOutcome};

    use crate::autostart::RunKeyAutostart;
    use crate::monitors::DesktopWallpaperMonitors;
    use crate::wallpaper::DesktopWallpaperSetter;

    /// The concrete scheduler used by the running app.
    pub type AppScheduler = Scheduler<DesktopWallpaperSetter, StdRng>;

    /// Central mutable state shared by main / tray / settings_ui.
    pub struct AppState {
        pub config: Config,
        pub config_path: PathBuf,
        /// Set when in-memory config diverges from disk; saved on Exit.
        pub dirty: bool,
        /// Live monitors from the most recent enumeration.
        pub monitors: Vec<MonitorInfo>,
        pub scheduler: AppScheduler,
        pub enumerator: DesktopWallpaperMonitors,
        pub autostart: RunKeyAutostart,
        /// Win32 timer parameters (owned/applied by `main`).
        pub interval_secs: u64,
        /// Top-level message-only window handle that owns the timer; raw
        /// `HWND` value stored as `isize` to keep this struct free of
        /// windows-crate types in its signature surface.
        pub hwnd: isize,
        /// Set by the tray "Exit" handler; `main`'s loop posts WM_QUIT.
        pub quit_requested: bool,
    }

    /// Build an assignment list from a monitor slice and config: available
    /// monitors only, each paired with its configured folder list (empty if
    /// unconfigured). Used both at startup (before `AppState` exists) and by
    /// `AppState::current_assignments` to avoid duplicating the same logic.
    pub fn build_assignments(
        monitors: &[MonitorInfo],
        config: &Config,
    ) -> Vec<(MonitorId, Vec<PathBuf>)> {
        monitors
            .iter()
            .filter(|m| m.available)
            .map(|m| {
                let folders = config
                    .monitors
                    .get(&m.id)
                    .map(|mc| mc.folders.clone())
                    .unwrap_or_default();
                (m.id.clone(), folders)
            })
            .collect()
    }

    impl AppState {
        pub fn new(
            config: Config,
            config_path: PathBuf,
            monitors: Vec<MonitorInfo>,
            scheduler: AppScheduler,
            enumerator: DesktopWallpaperMonitors,
            autostart: RunKeyAutostart,
        ) -> Self {
            let interval_secs = config.interval_secs;
            AppState {
                config,
                config_path,
                dirty: false,
                monitors,
                scheduler,
                enumerator,
                autostart,
                interval_secs,
                hwnd: 0,
                quit_requested: false,
            }
        }

        /// Build a fresh RNG-backed scheduler from a config + monitor list.
        /// Returns `None` when the COM wallpaper interface cannot be created
        /// (e.g. very early in startup before the shell is ready).
        pub fn build_scheduler(config: &Config) -> Option<AppScheduler> {
            let setter = DesktopWallpaperSetter::new().ok()?;
            let rng = StdRng::from_entropy();
            Some(Scheduler::new(
                setter,
                rng,
                config.cycle_mode,
                config.fit_mode,
            ))
        }

        /// Assignment list for the *currently live* monitors, looking up each
        /// monitor's folders by id (unknown monitors => empty / unconfigured).
        pub fn current_assignments(&self) -> Vec<(MonitorId, Vec<PathBuf>)> {
            build_assignments(&self.monitors, &self.config)
        }

        /// (Re)build the scheduler's playlists from the current config + live
        /// monitors, preserving assignments by `MonitorId`.
        pub fn rebuild_scheduler(&mut self) {
            let assignments = self.current_assignments();
            self.scheduler.rebuild(&assignments);
        }

        // ---- tray dispatch (§5.8) -------------------------------------

        /// Tray "Next": advance every monitor now, ignoring pause.
        pub fn on_next(&mut self) -> Vec<TickOutcome> {
            self.scheduler.next_now()
        }

        /// Tray "Pause/Resume": flip the pause flag; returns the new state so
        /// the menu label can update.
        pub fn on_toggle_pause(&mut self) -> bool {
            if self.scheduler.is_paused() {
                self.scheduler.resume();
            } else {
                self.scheduler.pause();
            }
            self.scheduler.is_paused()
        }

        pub fn is_paused(&self) -> bool {
            self.scheduler.is_paused()
        }

        /// Tray "Exit": request shutdown. The actual KillTimer / save /
        /// CoUninitialize / PostQuitMessage sequence is performed by `main`.
        pub fn on_exit(&mut self) {
            self.quit_requested = true;
        }

        // ---- timer tick ----------------------------------------------

        /// Timer fired: advance every monitor (no-op while paused).
        pub fn on_tick(&mut self) -> Vec<TickOutcome> {
            self.scheduler.tick()
        }

        // ---- settings dispatch (§5.9) --------------------------------

        /// Apply edited settings: persist config, then reconcile the
        /// scheduler/timer with what actually changed. Returns whether the
        /// timer interval changed (so `main` can KillTimer + SetTimer).
        ///
        /// `new_config` is the fully-formed edited config (already clamped by
        /// the settings UI). `folders_changed` indicates any per-monitor
        /// folder edit.
        pub fn apply_settings(
            &mut self,
            new_config: Config,
            folders_changed: bool,
        ) -> Result<SettingsApplyResult, wallpaper_shuffler_core::model::AppError> {
            let new_config = new_config.clamped();

            let interval_changed = new_config.interval_secs != self.config.interval_secs;
            let mode_changed = new_config.cycle_mode != self.config.cycle_mode;
            let fit_changed = new_config.fit_mode != self.config.fit_mode;
            let autostart_changed = new_config.autostart != self.config.autostart;

            // Drive autostart first so a failure surfaces before we commit.
            if autostart_changed {
                self.autostart.set_enabled(new_config.autostart)?;
            }

            self.config = new_config;
            self.interval_secs = self.config.interval_secs;

            config::save(&self.config_path, &self.config)?;
            self.dirty = false;

            if mode_changed {
                self.scheduler.set_mode(self.config.cycle_mode);
            }
            if fit_changed {
                self.scheduler.set_fit(self.config.fit_mode);
            }
            if folders_changed {
                self.rebuild_scheduler();
            }

            Ok(SettingsApplyResult {
                interval_changed,
                new_interval_secs: self.config.interval_secs,
            })
        }

        /// Current autostart state, read through the seam (for the checkbox).
        pub fn autostart_enabled(&self) -> bool {
            self.autostart.is_enabled().unwrap_or(self.config.autostart)
        }

        /// Re-enumerate live monitors after WM_DISPLAYCHANGE and rebuild
        /// playlists, preserving assignments by id. Errors are returned to
        /// `main` for logging; never panics.
        pub fn on_display_change(
            &mut self,
        ) -> Result<(), wallpaper_shuffler_core::model::AppError> {
            let live = self.enumerator.enumerate()?;
            self.monitors = live;
            self.rebuild_scheduler();
            Ok(())
        }
    }

    pub struct SettingsApplyResult {
        pub interval_changed: bool,
        pub new_interval_secs: u64,
    }

    /// Convenience alias for the shared handle passed to tray + settings_ui.
    pub type SharedState = Rc<std::cell::RefCell<AppState>>;

    // Keep the model trait/enum imports referenced so the module type-checks
    // even if some are only used through method calls above.
    #[allow(dead_code)]
    fn _assert_seam_types(_m: CycleMode, _f: FitMode, _e: &dyn MonitorEnumerator) {}
}

// ---------------------------------------------------------------------------
// Windows entry point (§7 full lifecycle).
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn main() {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    use windows::core::w;
    use windows::Win32::Foundation::{
        GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, KillTimer, PostQuitMessage,
        RegisterClassW, SetTimer, TranslateMessage, CW_USEDEFAULT, MSG, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_DISPLAYCHANGE, WM_TIMER, WNDCLASSW,
    };

    use app_state::{AppState, SharedState};

    const TIMER_ID: usize = 1;

    // ---- single-instance named-mutex guard (REQUIRED, §5.10) ----
    // Acquire BEFORE any other work; hold for process lifetime. A second
    // launch (e.g. autostart + manual start) exits immediately.
    let _singleton = unsafe {
        let handle = CreateMutexW(None, true, w!("Global\\wallpaper-shuffler-singleton"));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    // Another instance already holds it: exit silently.
                    return;
                }
                h
            }
            Err(_) => {
                // Can't create the guard — fail safe by exiting rather than
                // risk a competing shuffler.
                return;
            }
        }
    };

    // ---- resolve %APPDATA% paths (Windows-only) ----
    let appdata = match std::env::var_os("APPDATA") {
        Some(v) => PathBuf::from(v),
        None => {
            // Without APPDATA we have nowhere to persist; bail quietly.
            return;
        }
    };
    let app_dir = appdata.join("wallpaper-shuffler");
    let config_path = app_dir.join("config.toml");
    let log_path = app_dir.join("log.txt");

    // Best-effort: ensure the app dir exists for logging/config.
    let _ = std::fs::create_dir_all(&app_dir);

    // ---- COM init; never panic on failure (§8) ----
    let com_ok = unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        hr.is_ok()
    };
    if !com_ok {
        log_error(&log_path, "CoInitializeEx failed; exiting");
        return;
    }

    // ---- build real seam impls ----
    let enumerator = match crate::monitors::DesktopWallpaperMonitors::new() {
        Ok(e) => e,
        Err(e) => {
            log_error(
                &log_path,
                &format!("DesktopWallpaperMonitors init failed: {e:?}"),
            );
            unsafe { CoUninitialize() };
            return;
        }
    };
    let autostart = crate::autostart::RunKeyAutostart;

    // ---- load config (resolve %APPDATA%, §5.2) ----
    let (config, needs_settings) = wallpaper_shuffler_core::config::load_or_default(&config_path);
    let config = config.clamped();

    // On a reset/missing config, persist defaults so the autostart entry +
    // baseline settings exist (§7 step 2 / §defaults: autostart on first run).
    if needs_settings {
        if let Err(e) = wallpaper_shuffler_core::config::save(&config_path, &config) {
            log_error(&log_path, &format!("initial config save failed: {e:?}"));
        }
        // Default autostart = true: ensure the Run key is present on first run.
        if config.autostart {
            if let Err(e) = {
                use wallpaper_shuffler_core::model::AutostartManager;
                autostart.set_enabled(true)
            } {
                log_error(&log_path, &format!("autostart enable failed: {e:?}"));
            }
        }
    }

    // ---- enumerate live monitors (§7 step 3) ----
    let monitors = {
        use wallpaper_shuffler_core::model::MonitorEnumerator;
        match enumerator.enumerate() {
            Ok(m) => m,
            Err(e) => {
                log_error(&log_path, &format!("monitor enumerate failed: {e:?}"));
                Vec::new()
            }
        }
    };

    // ---- build scheduler (mode/fit from config) + playlists (§7 step 4-5) ----
    let mut scheduler = match AppState::build_scheduler(&config) {
        Some(s) => s,
        None => {
            log_error(&log_path, "DesktopWallpaperSetter init failed; exiting");
            unsafe { CoUninitialize() };
            return;
        }
    };
    {
        // Build initial assignments (AppState not yet constructed).
        let assignments = app_state::build_assignments(&monitors, &config);
        scheduler.rebuild(&assignments);
    }

    let state: SharedState = Rc::new(RefCell::new(AppState::new(
        config,
        config_path,
        monitors,
        scheduler,
        enumerator,
        autostart,
    )));

    // ---- create a message-only window to own the timer + receive messages ----
    let hinstance = unsafe {
        windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map(|h| h.into())
            .unwrap_or(windows::Win32::Foundation::HINSTANCE::default())
    };

    let class_name = w!("WallpaperShufflerWnd");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        lpszClassName: class_name,
        ..Default::default()
    };
    unsafe {
        RegisterClassW(&wc);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("wallpaper-shuffler"),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            hinstance,
            None,
        )
    };
    let hwnd = match hwnd {
        Ok(h) => h,
        Err(e) => {
            log_error(&log_path, &format!("CreateWindowExW failed: {e:?}"));
            unsafe { CoUninitialize() };
            return;
        }
    };

    // Stash hwnd + shared state pointer so the wndproc + tray can reach them.
    state.borrow_mut().hwnd = hwnd.0 as isize;
    set_global_state(state.clone());
    set_global_log_path(log_path.clone());

    // ---- create the tray (§5.8) ----
    let _tray = crate::tray::Tray::new(state.clone(), hwnd);

    // ---- start the timer (§7 step 5) ----
    let interval_ms = state.borrow().interval_secs.saturating_mul(1000) as u32;
    unsafe {
        SetTimer(hwnd, TIMER_ID, interval_ms.max(1000), None);
    }

    // ---- initial wallpaper set (§7 step 6: next_now once at startup) ----
    {
        let outcomes = state.borrow_mut().on_next();
        log_outcomes(&log_path, &outcomes);
    }

    // If config was reset, open settings so the user can configure folders.
    if needs_settings {
        crate::settings_ui::open(state.clone());
    }

    // ---- message loop (§7) ----
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);

            // Honor a tray-requested exit between message dispatches.
            if state.borrow().quit_requested {
                shutdown(&state, hwnd, TIMER_ID, &log_path);
                break;
            }
        }
    }

    unsafe { CoUninitialize() };
    drop(_singleton); // explicit: mutex held for process lifetime until here.

    // ---- nested helpers -------------------------------------------------

    /// Window procedure: timer ticks + display-change handling.
    extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER => {
                if let Some(state) = global_state() {
                    let outcomes = state.borrow_mut().on_tick();
                    if let Some(log) = global_log_path() {
                        log_outcomes(&log, &outcomes);
                    }
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                if let Some(state) = global_state() {
                    let res = state.borrow_mut().on_display_change();
                    if let (Err(e), Some(log)) = (res, global_log_path()) {
                        log_error(&log, &format!("display change failed: {e:?}"));
                    }
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// Shutdown sequence (§7 / §5.10 Exit): KillTimer, hide tray, save if
    /// dirty, then PostQuitMessage. CoUninitialize happens after the loop.
    fn shutdown(state: &SharedState, hwnd: HWND, timer_id: usize, log_path: &std::path::Path) {
        unsafe {
            let _ = KillTimer(hwnd, timer_id);
        }
        // Tray is hidden by dropping it (NWG removes the icon on Drop);
        // explicit hide is handled in tray on its own when present.
        let mut s = state.borrow_mut();
        if s.dirty {
            if let Err(e) = wallpaper_shuffler_core::config::save(&s.config_path, &s.config) {
                log_error(log_path, &format!("save on exit failed: {e:?}"));
            } else {
                s.dirty = false;
            }
        }
        unsafe {
            PostQuitMessage(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Global shared-state plumbing for the wndproc (which cannot capture state).
// All windows-only; cfg-stripped on Linux.
// ---------------------------------------------------------------------------

#[cfg(windows)]
thread_local! {
    static GLOBAL_STATE: std::cell::RefCell<Option<app_state::SharedState>> =
        const { std::cell::RefCell::new(None) };
    static GLOBAL_LOG: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
fn set_global_state(state: app_state::SharedState) {
    GLOBAL_STATE.with(|g| *g.borrow_mut() = Some(state));
}

#[cfg(windows)]
fn global_state() -> Option<app_state::SharedState> {
    GLOBAL_STATE.with(|g| g.borrow().clone())
}

#[cfg(windows)]
fn set_global_log_path(path: std::path::PathBuf) {
    GLOBAL_LOG.with(|g| *g.borrow_mut() = Some(path));
}

#[cfg(windows)]
fn global_log_path() -> Option<std::path::PathBuf> {
    GLOBAL_LOG.with(|g| g.borrow().clone())
}

// ---------------------------------------------------------------------------
// Logging (§8): append errors/outcomes to %APPDATA%\...\log.txt; never panic.
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn log_error(path: &std::path::Path, msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[error] {msg}");
    }
}

#[cfg(windows)]
fn log_outcomes(
    path: &std::path::Path,
    outcomes: &[wallpaper_shuffler_core::scheduler::TickOutcome],
) {
    use std::io::Write;
    use wallpaper_shuffler_core::scheduler::TickResult;
    let mut f = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(f) => f,
        Err(_) => return,
    };
    for o in outcomes {
        match &o.result {
            TickResult::Applied(p) => {
                let _ = writeln!(f, "[applied] {} -> {}", o.monitor, p.display());
            }
            TickResult::SkippedEmpty => {
                let _ = writeln!(f, "[skip-empty] {}", o.monitor);
            }
            TickResult::Failed(e) => {
                let _ = writeln!(f, "[failed] {} : {e}", o.monitor);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Non-Windows stub: the app is Windows-only. Keeps `cargo build`/`cargo test`
// green on Linux/CI.
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn main() {
    eprintln!("wallpaper-shuffler is Windows-only");
}
