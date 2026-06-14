// app::settings_ui — settings window (§5.9, Windows).
//
// Lists detected monitors with orientation labels + an unconfigured flag,
// per-monitor folder add/remove, global interval (minutes, min 1), global cycle
// mode, global fit mode, and an autostart toggle. On apply: mutate Config ->
// config::save -> reconcile timer / scheduler. Closing the window keeps the app
// alive in the tray.
//
// Everything below is `#[cfg(windows)]`-gated so this file parses (compiling to
// nothing) on Linux after cfg-stripping.

#[cfg(windows)]
pub use imp::open;

/// Non-Windows no-op so callers compile on Linux (they are themselves gated,
/// but keeping a stub here documents the surface).
#[cfg(not(windows))]
#[allow(dead_code)]
pub fn open(_state: ()) {}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use native_windows_gui as nwg;

    use wallpaper_shuffler_core::config::{Config, MonitorConfig, MIN_INTERVAL_SECS};
    use wallpaper_shuffler_core::model::{CycleMode, FitMode, MonitorInfo, Orientation};

    use crate::app_state::SettingsApplyResult;
    use crate::app_state::SharedState;

    /// Open (or focus) the settings window. The window is modeless and owns its
    /// own NWG controls; closing it just hides/destroys the window, leaving the
    /// tray app running.
    pub fn open(state: SharedState) {
        let _ = nwg::init();

        let mut ui = Box::<SettingsUi>::default();

        // Snapshot the current monitors + config for initial population.
        let (monitors, config) = {
            let s = state.borrow();
            (s.monitors.clone(), s.config.clone())
        };
        let autostart_on = state.borrow().autostart_enabled();

        build_window(&mut ui);
        *ui.state.borrow_mut() = Some(state.clone());
        ui.monitors.replace(monitors.clone());

        populate_monitor_list(&ui, &monitors, &config);
        populate_globals(&ui, &config, autostart_on);

        let window_handle = ui.window.handle;
        let h_add = ui.add_folder_btn.handle;
        let h_remove = ui.remove_folder_btn.handle;
        let h_apply = ui.apply_btn.handle;
        let h_close = ui.close_btn.handle;
        let win = ui.window.handle;

        let ui_ptr: *const SettingsUi = &*ui;

        let handler = nwg::full_bind_event_handler(&window_handle, move |evt, _data, handle| {
            use nwg::Event as E;
            // SAFETY: ui outlives the handler; both are stored in the leaked
            // window controller below for the window's lifetime.
            let ui: &SettingsUi = unsafe { &*ui_ptr };
            match evt {
                E::OnButtonClick => {
                    if handle == h_add {
                        on_add_folder(ui);
                    } else if handle == h_remove {
                        on_remove_folder(ui);
                    } else if handle == h_apply {
                        on_apply(ui);
                    } else if handle == h_close {
                        // Closing keeps the app in the tray: just hide.
                        ui.window.set_visible(false);
                    }
                }
                E::OnWindowClose if handle == win => {
                    // Just hide; the app keeps running in the tray.
                    ui.window.set_visible(false);
                }
                _ => {}
            }
        });

        ui.window.set_visible(true);

        // Keep the window + handler alive for the window's lifetime by leaking
        // into a thread-local registry; cleaned up when the window closes.
        register_window(SettingsController {
            _ui: ui,
            _handler: handler,
        });
    }

    #[derive(Default)]
    struct SettingsUi {
        window: nwg::Window,
        layout: nwg::GridLayout,

        monitor_list: nwg::ListBox<String>,
        folder_list: nwg::ListBox<String>,
        add_folder_btn: nwg::Button,
        remove_folder_btn: nwg::Button,

        interval_label: nwg::Label,
        interval_input: nwg::TextInput,

        cycle_label: nwg::Label,
        cycle_combo: nwg::ComboBox<&'static str>,

        fit_label: nwg::Label,
        fit_combo: nwg::ComboBox<&'static str>,

        autostart_check: nwg::CheckBox,

        apply_btn: nwg::Button,
        close_btn: nwg::Button,

        folder_dialog: nwg::FileDialog,

        state: RefCell<Option<SharedState>>,
        /// Live monitors shown in the list (index-aligned with monitor_list).
        monitors: RefCell<Vec<MonitorInfo>>,
        /// Working copy of per-monitor folders, edited before Apply.
        /// Keyed positionally to `monitors`.
        working_folders: RefCell<Vec<Vec<PathBuf>>>,
    }

    const CYCLE_ITEMS: [&str; 3] = ["Sequential", "Shuffle", "PureRandom"];
    const FIT_ITEMS: [&str; 5] = ["Fill", "Fit", "Stretch", "Center", "Tile"];

    fn build_window(ui: &mut SettingsUi) {
        nwg::Window::builder()
            .size((520, 420))
            .position((300, 200))
            .title("wallpaper-shuffler settings")
            .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
            .build(&mut ui.window)
            .expect("settings window");

        nwg::ListBox::builder()
            .parent(&ui.window)
            .build(&mut ui.monitor_list)
            .expect("monitor list");

        nwg::ListBox::builder()
            .parent(&ui.window)
            .build(&mut ui.folder_list)
            .expect("folder list");

        nwg::Button::builder()
            .text("Add folder")
            .parent(&ui.window)
            .build(&mut ui.add_folder_btn)
            .expect("add folder btn");

        nwg::Button::builder()
            .text("Remove folder")
            .parent(&ui.window)
            .build(&mut ui.remove_folder_btn)
            .expect("remove folder btn");

        nwg::Label::builder()
            .text("Interval (minutes, min 1):")
            .parent(&ui.window)
            .build(&mut ui.interval_label)
            .expect("interval label");

        nwg::TextInput::builder()
            .parent(&ui.window)
            .build(&mut ui.interval_input)
            .expect("interval input");

        nwg::Label::builder()
            .text("Cycle mode:")
            .parent(&ui.window)
            .build(&mut ui.cycle_label)
            .expect("cycle label");

        nwg::ComboBox::builder()
            .collection(CYCLE_ITEMS.to_vec())
            .parent(&ui.window)
            .build(&mut ui.cycle_combo)
            .expect("cycle combo");

        nwg::Label::builder()
            .text("Fit mode:")
            .parent(&ui.window)
            .build(&mut ui.fit_label)
            .expect("fit label");

        nwg::ComboBox::builder()
            .collection(FIT_ITEMS.to_vec())
            .parent(&ui.window)
            .build(&mut ui.fit_combo)
            .expect("fit combo");

        nwg::CheckBox::builder()
            .text("Start at login (autostart)")
            .parent(&ui.window)
            .build(&mut ui.autostart_check)
            .expect("autostart check");

        nwg::Button::builder()
            .text("Apply")
            .parent(&ui.window)
            .build(&mut ui.apply_btn)
            .expect("apply btn");

        nwg::Button::builder()
            .text("Close")
            .parent(&ui.window)
            .build(&mut ui.close_btn)
            .expect("close btn");

        nwg::FileDialog::builder()
            .title("Select a wallpaper folder")
            .action(nwg::FileDialogAction::OpenDirectory)
            .build(&mut ui.folder_dialog)
            .expect("folder dialog");

        // Simple grid placement.
        nwg::GridLayout::builder()
            .parent(&ui.window)
            .spacing(2)
            .child_item(nwg::GridLayoutItem::new(&ui.monitor_list, 0, 0, 2, 4))
            .child_item(nwg::GridLayoutItem::new(&ui.folder_list, 2, 0, 3, 3))
            .child_item(nwg::GridLayoutItem::new(&ui.add_folder_btn, 2, 3, 1, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.remove_folder_btn, 3, 3, 1, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.interval_label, 0, 4, 2, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.interval_input, 2, 4, 2, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.cycle_label, 0, 5, 2, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.cycle_combo, 2, 5, 2, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.fit_label, 0, 6, 2, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.fit_combo, 2, 6, 2, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.autostart_check, 0, 7, 3, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.apply_btn, 3, 7, 1, 1))
            .child_item(nwg::GridLayoutItem::new(&ui.close_btn, 4, 7, 1, 1))
            .build(&ui.layout)
            .expect("settings layout");
    }

    fn orientation_label(o: Orientation) -> &'static str {
        match o {
            Orientation::Landscape => "Landscape",
            Orientation::Portrait => "Portrait",
        }
    }

    fn populate_monitor_list(ui: &SettingsUi, monitors: &[MonitorInfo], config: &Config) {
        let mut entries = Vec::with_capacity(monitors.len());
        let mut working = Vec::with_capacity(monitors.len());
        for m in monitors {
            let folders = config
                .monitors
                .get(&m.id)
                .map(|mc| mc.folders.clone())
                .unwrap_or_default();
            let unconfigured = folders.is_empty();
            let label = format!(
                "{} [{}]{}",
                short_id(&m.id),
                orientation_label(m.orientation),
                if unconfigured { " (unconfigured)" } else { "" }
            );
            entries.push(label);
            working.push(folders);
        }
        ui.monitor_list.set_collection(entries);
        ui.working_folders.replace(working);
        // Select the first monitor by default and show its folders.
        if !monitors.is_empty() {
            ui.monitor_list.set_selection(Some(0));
            show_folders_for_selected(ui);
        }
    }

    /// Trim the long device-path id to something readable in the list.
    fn short_id(id: &str) -> String {
        // device paths look like \\?\DISPLAY#DELA1B2#...; show the middle token.
        id.split('#')
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let t = id.trim_start_matches(['\\', '?', '.']);
                t.chars().take(24).collect()
            })
    }

    fn selected_monitor_index(ui: &SettingsUi) -> Option<usize> {
        ui.monitor_list.selection()
    }

    fn show_folders_for_selected(ui: &SettingsUi) {
        if let Some(idx) = selected_monitor_index(ui) {
            let working = ui.working_folders.borrow();
            if let Some(folders) = working.get(idx) {
                let items: Vec<String> = folders.iter().map(|p| p.display().to_string()).collect();
                ui.folder_list.set_collection(items);
            }
        }
    }

    fn populate_globals(ui: &SettingsUi, config: &Config, autostart_on: bool) {
        // Interval in minutes (min 1).
        let minutes = (config.interval_secs / 60).max(1);
        ui.interval_input.set_text(&minutes.to_string());

        let cycle_idx = match config.cycle_mode {
            CycleMode::Sequential => 0,
            CycleMode::Shuffle => 1,
            CycleMode::PureRandom => 2,
        };
        ui.cycle_combo.set_selection(Some(cycle_idx));

        let fit_idx = match config.fit_mode {
            FitMode::Fill => 0,
            FitMode::Fit => 1,
            FitMode::Stretch => 2,
            FitMode::Center => 3,
            FitMode::Tile => 4,
        };
        ui.fit_combo.set_selection(Some(fit_idx));

        ui.autostart_check.set_check_state(if autostart_on {
            nwg::CheckBoxState::Checked
        } else {
            nwg::CheckBoxState::Unchecked
        });
    }

    fn on_add_folder(ui: &SettingsUi) {
        let Some(idx) = selected_monitor_index(ui) else {
            return;
        };
        if ui.folder_dialog.run(Some(&ui.window)) {
            if let Ok(item) = ui.folder_dialog.get_selected_item() {
                let path = PathBuf::from(item.to_string_lossy().to_string());
                {
                    let mut working = ui.working_folders.borrow_mut();
                    if let Some(folders) = working.get_mut(idx) {
                        if !folders.contains(&path) {
                            folders.push(path);
                        }
                    }
                }
                show_folders_for_selected(ui);
                refresh_monitor_label(ui, idx);
            }
        }
    }

    fn on_remove_folder(ui: &SettingsUi) {
        let Some(midx) = selected_monitor_index(ui) else {
            return;
        };
        let Some(fidx) = ui.folder_list.selection() else {
            return;
        };
        {
            let mut working = ui.working_folders.borrow_mut();
            if let Some(folders) = working.get_mut(midx) {
                if fidx < folders.len() {
                    folders.remove(fidx);
                }
            }
        }
        show_folders_for_selected(ui);
        refresh_monitor_label(ui, midx);
    }

    /// Refresh a single monitor's label (the unconfigured flag may have changed).
    fn refresh_monitor_label(ui: &SettingsUi, idx: usize) {
        let monitors = ui.monitors.borrow();
        let working = ui.working_folders.borrow();
        if let (Some(m), Some(folders)) = (monitors.get(idx), working.get(idx)) {
            let unconfigured = folders.is_empty();
            let label = format!(
                "{} [{}]{}",
                short_id(&m.id),
                orientation_label(m.orientation),
                if unconfigured { " (unconfigured)" } else { "" }
            );
            // Rebuild the whole collection (NWG ListBox has no per-row set).
            let mut entries: Vec<String> = (0..monitors.len())
                .map(|i| {
                    if i == idx {
                        label.clone()
                    } else {
                        let mm = &monitors[i];
                        let f = &working[i];
                        format!(
                            "{} [{}]{}",
                            short_id(&mm.id),
                            orientation_label(mm.orientation),
                            if f.is_empty() { " (unconfigured)" } else { "" }
                        )
                    }
                })
                .collect();
            let sel = ui.monitor_list.selection();
            ui.monitor_list.set_collection(std::mem::take(&mut entries));
            ui.monitor_list.set_selection(sel);
        }
    }

    fn parse_interval_minutes(ui: &SettingsUi) -> u64 {
        let text = ui.interval_input.text();
        let minutes: u64 = text.trim().parse().unwrap_or(0);
        let minutes = minutes.max(1); // min 1 minute in the UI
        (minutes * 60).max(MIN_INTERVAL_SECS)
    }

    fn selected_cycle(ui: &SettingsUi) -> CycleMode {
        match ui.cycle_combo.selection().unwrap_or(0) {
            1 => CycleMode::Shuffle,
            2 => CycleMode::PureRandom,
            _ => CycleMode::Sequential,
        }
    }

    fn selected_fit(ui: &SettingsUi) -> FitMode {
        match ui.fit_combo.selection().unwrap_or(0) {
            1 => FitMode::Fit,
            2 => FitMode::Stretch,
            3 => FitMode::Center,
            4 => FitMode::Tile,
            _ => FitMode::Fill,
        }
    }

    fn on_apply(ui: &SettingsUi) {
        let Some(state) = ui.state.borrow().as_ref().cloned() else {
            return;
        };

        // Build the edited config from the existing config + UI fields.
        let mut new_config = state.borrow().config.clone();
        new_config.interval_secs = parse_interval_minutes(ui);
        new_config.cycle_mode = selected_cycle(ui);
        new_config.fit_mode = selected_fit(ui);
        new_config.autostart = ui.autostart_check.check_state() == nwg::CheckBoxState::Checked;

        // Detect folder changes + write the working folders back into config.
        let monitors = ui.monitors.borrow();
        let working = ui.working_folders.borrow();
        let mut folders_changed = false;
        for (i, m) in monitors.iter().enumerate() {
            let new_folders = working.get(i).cloned().unwrap_or_default();
            let old_folders = new_config
                .monitors
                .get(&m.id)
                .map(|mc| mc.folders.clone())
                .unwrap_or_default();
            if new_folders != old_folders {
                folders_changed = true;
            }
            if new_folders.is_empty() {
                // Keep an explicit (empty) entry so the monitor is known.
                new_config
                    .monitors
                    .entry(m.id.clone())
                    .or_insert_with(MonitorConfig::default)
                    .folders = Vec::new();
            } else {
                new_config
                    .monitors
                    .entry(m.id.clone())
                    .or_insert_with(MonitorConfig::default)
                    .folders = new_folders;
            }
        }
        drop(monitors);
        drop(working);

        // Dispatch to the controller, which persists + reconciles scheduler.
        let result = state
            .borrow_mut()
            .apply_settings(new_config, folders_changed);
        match result {
            Ok(SettingsApplyResult {
                interval_changed,
                new_interval_secs,
            }) => {
                if interval_changed {
                    restart_timer(&state, new_interval_secs);
                }
            }
            Err(e) => {
                nwg::modal_error_message(
                    &ui.window,
                    "Settings",
                    &format!("Failed to apply settings: {e:?}"),
                );
            }
        }
    }

    /// KillTimer + SetTimer on the owner window when the interval changed.
    fn restart_timer(state: &SharedState, new_interval_secs: u64) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
        const TIMER_ID: usize = 1;
        let hwnd_raw = state.borrow().hwnd;
        if hwnd_raw == 0 {
            return;
        }
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        let interval_ms = new_interval_secs.saturating_mul(1000) as u32;
        unsafe {
            let _ = KillTimer(hwnd, TIMER_ID);
            SetTimer(hwnd, TIMER_ID, interval_ms.max(1000), None);
        }
    }

    // ---- window lifetime registry ------------------------------------

    /// Holds the boxed UI + its event handler so they live as long as the
    /// window is open. Multiple opens reuse a single slot (re-opening focuses
    /// the existing window by replacing the controller).
    struct SettingsController {
        _ui: Box<SettingsUi>,
        _handler: nwg::EventHandler,
    }

    thread_local! {
        static SETTINGS_WINDOW: RefCell<Option<SettingsController>> =
            const { RefCell::new(None) };
    }

    fn register_window(controller: SettingsController) {
        SETTINGS_WINDOW.with(|w| {
            *w.borrow_mut() = Some(controller);
        });
    }
}
