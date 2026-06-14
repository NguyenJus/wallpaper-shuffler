// app::tray — tray icon + context menu (§5.8, Windows).
//
// Exactly four menu items: Next, Pause/Resume (label toggles with state),
// Settings, Exit. Each dispatches into the shared `AppState` controller.
//
// Everything below is `#[cfg(windows)]`-gated so this file parses (but compiles
// to nothing) on Linux after cfg-stripping.

#[cfg(windows)]
pub use imp::Tray;

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;

    use native_windows_gui as nwg;
    use windows::Win32::Foundation::HWND;

    use crate::app_state::SharedState;

    /// Owns the NWG tray icon + context menu. Held for the app's lifetime;
    /// dropping it removes the tray icon.
    pub struct Tray {
        // Boxed UI struct so internal handles have a stable address for the
        // NWG event handler binding.
        _ui: Box<TrayUi>,
        _handler: nwg::EventHandler,
    }

    #[derive(Default)]
    struct TrayUi {
        window: nwg::MessageWindow,
        icon: nwg::Icon,
        tray: nwg::TrayNotification,
        menu: nwg::Menu,
        item_next: nwg::MenuItem,
        item_pause: nwg::MenuItem,
        item_settings: nwg::MenuItem,
        item_exit: nwg::MenuItem,
        state: RefCell<Option<SharedState>>,
        /// Handle to the message-only window owning the timer (for KillTimer /
        /// SetTimer when settings change).
        owner_hwnd: RefCell<isize>,
    }

    impl Tray {
        /// Build the tray icon + menu and wire each item to the controller.
        /// `owner_hwnd` is the message-only window that owns the Win32 timer.
        pub fn new(state: SharedState, owner_hwnd: HWND) -> Tray {
            // NWG must be initialised before any control is created. Safe to
            // call once; the app's single-instance guard ensures one process.
            let _ = nwg::init();

            let mut ui = Box::<TrayUi>::default();

            // A default application icon; the real .ico can be embedded later.
            let _ = nwg::Icon::builder()
                .source_system(Some(nwg::OemIcon::Sample))
                .build(&mut ui.icon);

            nwg::MessageWindow::builder()
                .build(&mut ui.window)
                .expect("tray message window");

            nwg::TrayNotification::builder()
                .parent(&ui.window)
                .icon(Some(&ui.icon))
                .tip(Some("wallpaper-shuffler"))
                .build(&mut ui.tray)
                .expect("tray notification");

            nwg::Menu::builder()
                .popup(true)
                .parent(&ui.window)
                .build(&mut ui.menu)
                .expect("tray menu");

            nwg::MenuItem::builder()
                .text("Next")
                .parent(&ui.menu)
                .build(&mut ui.item_next)
                .expect("menu: Next");

            let pause_label = if state.borrow().is_paused() {
                "Resume"
            } else {
                "Pause"
            };
            nwg::MenuItem::builder()
                .text(pause_label)
                .parent(&ui.menu)
                .build(&mut ui.item_pause)
                .expect("menu: Pause/Resume");

            nwg::MenuItem::builder()
                .text("Settings")
                .parent(&ui.menu)
                .build(&mut ui.item_settings)
                .expect("menu: Settings");

            nwg::MenuItem::builder()
                .text("Exit")
                .parent(&ui.menu)
                .build(&mut ui.item_exit)
                .expect("menu: Exit");

            *ui.state.borrow_mut() = Some(state);
            *ui.owner_hwnd.borrow_mut() = owner_hwnd.0 as isize;

            // Capture handles for the event closure.
            let window_handle = ui.window.handle;
            let menu = ui.menu.handle;
            let tray = ui.tray.handle;
            let h_next = ui.item_next.handle;
            let h_pause = ui.item_pause.handle;
            let h_settings = ui.item_settings.handle;
            let h_exit = ui.item_exit.handle;

            // Raw pointer to the UI so the closure can reach the controls
            // without moving the box (the box lives in `Tray`).
            let ui_ptr: *const TrayUi = &*ui;

            let handler =
                nwg::full_bind_event_handler(&window_handle, move |evt, _data, handle| {
                    use nwg::Event as E;
                    // SAFETY: `ui` (and thus `*ui_ptr`) outlives the handler; both
                    // are owned by the returned `Tray` and dropped together.
                    let ui: &TrayUi = unsafe { &*ui_ptr };

                    match evt {
                        // Right-click (or the contextual gesture) on the tray icon
                        // pops the menu at the cursor.
                        E::OnContextMenu if handle == tray => {
                            let (x, y) = nwg::GlobalCursor::position();
                            ui.menu.popup(x, y);
                        }
                        E::OnMenuItemSelected => {
                            if handle == h_next {
                                dispatch_next(ui);
                            } else if handle == h_pause {
                                dispatch_toggle_pause(ui);
                            } else if handle == h_settings {
                                dispatch_settings(ui);
                            } else if handle == h_exit {
                                dispatch_exit(ui);
                            }
                        }
                        _ => {
                            let _ = menu; // keep capture referenced
                        }
                    }
                });

            Tray {
                _ui: ui,
                _handler: handler,
            }
        }
    }

    fn with_state<F: FnOnce(&SharedState)>(ui: &TrayUi, f: F) {
        if let Some(state) = ui.state.borrow().as_ref() {
            f(state);
        }
    }

    fn dispatch_next(ui: &TrayUi) {
        with_state(ui, |state| {
            let outcomes = state.borrow_mut().on_next();
            if let Some(log) = crate::global_log_path() {
                crate::log_outcomes(&log, &outcomes);
            }
        });
    }

    fn dispatch_toggle_pause(ui: &TrayUi) {
        with_state(ui, |state| {
            let now_paused = state.borrow_mut().on_toggle_pause();
            // Update the menu label to reflect the new state via Win32 ModifyMenuW,
            // since NWG's MenuItem has no set_text method.
            set_menu_item_text(&ui.item_pause, if now_paused { "Resume" } else { "Pause" });
        });
    }

    /// Change a MenuItem's display text in-place using Win32 ModifyMenuW.
    /// The item is identified by command-id (MF_BYCOMMAND) from the NWG handle.
    fn set_menu_item_text(item: &nwg::MenuItem, text: &str) {
        use windows::Win32::UI::WindowsAndMessaging::{
            ModifyMenuW, HMENU, MF_BYCOMMAND, MF_STRING,
        };
        // NWG stores MenuItem handles as ControlHandle::MenuItem(parent_hmenu, item_id).
        // Both HMENU types (winapi and windows-0.58) are *mut opaque, ABI-compatible.
        if let Some((parent_hmenu, item_id)) = item.handle.hmenu_item() {
            // Build a NUL-terminated UTF-16 string for the new label.
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
            let pcwstr = windows::core::PCWSTR(wide.as_ptr());
            // Cast the winapi HMENU (*mut HMENU__) to windows-0.58 HMENU (*mut c_void).
            let hmenu = HMENU(parent_hmenu as *mut std::ffi::c_void);
            unsafe {
                let _ = ModifyMenuW(
                    hmenu,
                    item_id,
                    MF_BYCOMMAND | MF_STRING,
                    item_id as usize,
                    pcwstr,
                );
            }
        }
    }

    fn dispatch_settings(ui: &TrayUi) {
        with_state(ui, |state| {
            crate::settings_ui::open(state.clone());
        });
    }

    fn dispatch_exit(ui: &TrayUi) {
        with_state(ui, |state| {
            state.borrow_mut().on_exit();
        });
        // `on_exit` sets quit_requested; the main message loop performs the
        // KillTimer / save / PostQuitMessage sequence on its next iteration.
        // Nudge the loop so it re-checks promptly.
        let owner = *ui.owner_hwnd.borrow();
        if owner != 0 {
            unsafe {
                use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_NULL};
                let _ = PostMessageW(
                    HWND(owner as *mut std::ffi::c_void),
                    WM_NULL,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}
