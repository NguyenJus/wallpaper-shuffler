# wallpaper-shuffler — Implementation Spec

Date: 2026-06-14
Status: Locked design → implementation-ready spec
Audience: an implementation session doing TDD.

---

## 1. Overview & Goals

`wallpaper-shuffler` is a tiny Rust system-tray app for **Windows 11** that
rotates desktop wallpapers across multiple monitors. Free/open-source.

**Overriding priority: smallest binary + lowest RAM above everything else.**
When a design choice trades convenience for binary/RAM size, choose size.

Core behaviors:

- Multiple monitors with **different orientations**, each getting an
  independent, correctly-oriented wallpaper.
- Image bank = local folder(s), assigned **per monitor** (PER-MONITOR FOLDERS
  model). The user puts portrait images in a portrait monitor's folder and
  landscape images in a landscape monitor's folder. **There is no image
  rotation / transform logic** — orientation is handled by construction.
- Three cycle patterns: **Sequential** (fixed order, sorted by filename),
  **Shuffle** (random, no repeat until the folder is exhausted), **Pure
  random** (random each tick, repeats allowed).
- Frequency (interval) and cycle mode are **GLOBAL** — one interval and one
  mode for all monitors; all monitors advance together on the same tick.
  **Folders are per-monitor.**
- Runs in the background from the system tray.
- **Auto-start at login** (toggleable). Tray quick controls: **Next**,
  **Pause/Resume**, **Settings**, **Exit**.

Single `.exe`, no runtime dependency.

---

## 2. Non-goals (YAGNI — explicit)

The following are **out of scope** and must not be built:

- Live / animated wallpapers.
- Online / remote image sources.
- Auto orientation-matching from a shared pool (no automatic sorting of a mixed
  folder into the right monitor by aspect ratio).
- Spanning one image across multiple monitors.
- Per-monitor independent timers (the timer is global).
- Image cropping / editing / rotation / any pixel transform.
- WebP and any format outside the supported set (§ "Supported image formats").

---

## 3. Shared constraints (defined once; referenced by name)

### 3.1 Release profile (§release-profile)

`Cargo.toml` release profile — tuned for smallest binary:

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

Size is validated with `cargo build --release` and inspecting the produced
`.exe` size (see Manual verification checklist).

### 3.2 Trait-seam rule (§trait-seam)

**All Win32 / COM / registry / OS access lives behind a small trait**, with two
implementations:

1. a real `#[cfg(windows)]` implementation that calls the OS, and
2. a mock / fake used by unit tests on any platform.

**Pure-logic modules must never reference `windows`-crate types directly.**
They depend only on the traits and plain data types. This is what lets the
pure-logic crate compile and its tests run on **Linux/CI** before the developer
reaches a Windows machine. Windows-only code is gated behind `#[cfg(windows)]`
and behind these traits.

### 3.3 Supported image formats (§image-formats)

Case-insensitive extensions: `jpg`, `jpeg`, `png`, `bmp`. **`webp` is
explicitly excluded.** Anything else is ignored during folder scans.

### 3.4 Defaults (§defaults)

- Default interval: **30 minutes**. Minimum interval: **1 minute**. (Stored as
  seconds; see config schema.)
- Default fit mode: **Fill**.
- Default cycle mode: **Sequential**.
- Default autostart: **off** (false).

---

## 4. Crate layout & dependencies

### 4.1 Workspace layout

Two-crate Cargo workspace so pure logic compiles/tests on any OS:

```
wallpaper-shuffler/
├─ Cargo.toml                 # [workspace]
├─ crates/
│  ├─ core/                   # pure logic — builds & tests on Linux/CI
│  │  ├─ Cargo.toml
│  │  └─ src/
│  │     ├─ lib.rs
│  │     ├─ config.rs
│  │     ├─ playlist.rs
│  │     ├─ scheduler.rs
│  │     └─ model.rs          # shared plain-data types + traits (seams)
│  └─ app/                    # the Windows binary (the .exe)
│     ├─ Cargo.toml
│     └─ src/
│        ├─ main.rs           # wiring + Win32 message loop
│        ├─ monitors.rs       # IDesktopWallpaper enumeration impl
│        ├─ wallpaper.rs      # IDesktopWallpaper SetWallpaper/SetPosition impl
│        ├─ tray.rs           # NWG tray icon + menu
│        ├─ settings_ui.rs    # NWG settings window
│        └─ autostart.rs      # HKCU Run registry impl
```

Rationale: the **`core`** crate has no Windows dependency at all, so
`cargo test` runs everywhere. The **`app`** crate pulls in `windows` and
`native-windows-gui` and is only built/run on Windows.

> Recommended default; the implementer may collapse into a single crate using
> `#[cfg(windows)]` feature gates instead, **provided** `cargo test` for the
> pure-logic modules still runs on Linux/CI. The two-crate split is the
> lower-risk way to guarantee that.

### 4.2 Dependencies (recommended versions — implementer may bump)

`crates/core/Cargo.toml`:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.8"
rand = "0.8"          # PRNG for Shuffle / Pure random

[dev-dependencies]
tempfile = "3"        # temp folders for playlist folder-scan tests
```

`crates/app/Cargo.toml` (Windows-only — gate the whole crate's build to
Windows, or gate the deps with `[target.'cfg(windows)'.dependencies]`):

```toml
[dependencies]
wallpaper-shuffler-core = { path = "../core" }
serde = { version = "1", features = ["derive"] }
toml = "0.8"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = [
  "Win32_Foundation",
  "Win32_System_Com",
  "Win32_UI_Shell",                  # IDesktopWallpaper
  "Win32_UI_WindowsAndMessaging",    # message loop, WM_DISPLAYCHANGE
  "Win32_System_Registry",           # HKCU Run (or use the `winreg` crate)
] }
native-windows-gui = "1"
native-windows-derive = "1"          # optional; for NWG derive macros
```

> The exact `windows` feature list will be refined by the compiler errors
> during integration; the above is the recommended starting set. `winreg`
> ("0.52") is an acceptable smaller alternative for the registry work if it
> reduces binary size — implementer's choice, measured against §release-profile.

---

## 5. Module-by-module spec

Each module lists: responsibility, public API (Rust signatures), dependencies
on other modules, and how it is tested. Signatures are **recommended
defaults** — the implementer may adjust names/shapes as long as the
responsibilities, the §trait-seam rule, and the testability hold.

### 5.1 `core::model` — shared data types + trait seams

**Responsibility:** plain-data enums/structs shared across modules, and the
trait seams (§trait-seam) that isolate Windows code. No Windows deps.

```rust
/// Stable monitor identity = the Windows device-path string. Survives
/// reboot/replug, so config keys to it.
pub type MonitorId = String;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Orientation { Landscape, Portrait }

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum CycleMode { Sequential, Shuffle, PureRandom }

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum FitMode { Fill, Fit, Stretch, Center, Tile }

/// A live monitor as enumerated from the OS.
#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,   // derived: height > width => Portrait
}

/// SEAM: enumerate monitors. Real impl uses IDesktopWallpaper; tests use a fake.
pub trait MonitorEnumerator {
    fn enumerate(&self) -> Result<Vec<MonitorInfo>, AppError>;
}

/// SEAM: apply a wallpaper image to one monitor with a fit mode.
pub trait WallpaperSetter {
    fn set(&self, monitor: &MonitorId, image: &std::path::Path, fit: FitMode)
        -> Result<(), AppError>;
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
    Os(String),          // COM/Win32/registry failures, message-formatted
}
```

`Orientation` is derived as `Portrait` when `height > width`, else `Landscape`.
It is **only used as a label** in the settings UI.

**Tested:** trivial; covered indirectly by consumers. A direct unit test
asserts the `Orientation` derivation helper.

### 5.2 `core::config` — data model + TOML load/save

**Responsibility:** the persisted config model; load/save TOML; defaults;
corrupt/missing handling.

Config path: `%APPDATA%\wallpaper-shuffler\config.toml`. The **path-resolution**
function is Windows-only (`%APPDATA%`); config load/save take a path argument so
they are testable on Linux against temp files.

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    pub interval_secs: u64,                 // default 1800 (30 min); clamp >= 60
    pub cycle_mode: CycleMode,              // default Sequential
    pub fit_mode: FitMode,                  // default Fill
    pub autostart: bool,                    // default false
    /// Per-monitor folder assignments, keyed by stable MonitorId.
    pub monitors: std::collections::BTreeMap<MonitorId, MonitorConfig>,
}

#[derive(Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// One or more folders assigned to this monitor. Empty => unconfigured.
    pub folders: Vec<std::path::PathBuf>,
}

pub const MIN_INTERVAL_SECS: u64 = 60;
pub const DEFAULT_INTERVAL_SECS: u64 = 1800;

impl Default for Config { /* applies §defaults */ }

impl Config {
    /// Clamp interval to >= MIN_INTERVAL_SECS. Call after load and after edits.
    pub fn clamped(self) -> Config;

    /// Parse TOML text. Errors => caller writes defaults + opens settings.
    pub fn from_toml(text: &str) -> Result<Config, AppError>;
    pub fn to_toml(&self) -> Result<String, AppError>;
}

/// Load from disk; on missing OR parse-error, return (Config::default(), true)
/// where the bool = "was reset/needs settings". On success => (cfg, false).
pub fn load_or_default(path: &std::path::Path) -> (Config, bool);

/// Atomic-ish save: write to temp then rename. Creates parent dir.
pub fn save(path: &std::path::Path, cfg: &Config) -> Result<(), AppError>;
```

Use `#[serde(default)]` so partial/older config files fill missing fields from
`Default` rather than failing (forward/backward compatibility, fewer "corrupt"
resets).

**Dependencies:** `core::model`.

**Tested (TDD, unit):**
- `Config::default()` matches §defaults.
- `to_toml` → `from_toml` round-trip preserves a populated config.
- `from_toml` on garbage → `Err`; `load_or_default` on a garbage file →
  `(default, true)`; on a missing file → `(default, true)`.
- `clamped()` raises a sub-minimum interval to 60 and leaves valid ones.
- Unknown/missing fields tolerated via `serde(default)`.
- `save` then `load_or_default` round-trips through a real temp file
  (`tempfile`).

### 5.3 `core::playlist` — per-monitor image list + cursor

**Responsibility:** scan a monitor's assigned folders for supported images
(§image-formats), hold the resulting list + a cursor, and produce the next
image according to the active `CycleMode`. Skips corrupt/unreadable images by
advancing to the next entry (the *file-open* failure is detected by the
`WallpaperSetter`, which signals back so the playlist can skip — see Data flow).

```rust
pub struct Playlist {
    images: Vec<std::path::PathBuf>,   // sorted by filename for Sequential
    cursor: usize,                     // Sequential / Shuffle position
    shuffle_order: Vec<usize>,         // current shuffle permutation
}

impl Playlist {
    /// Scan all folders, collect supported images, sort by file name
    /// (case-insensitive, stable). Missing/empty folder contributes nothing.
    /// An empty result is valid (=> next() returns None).
    pub fn build(folders: &[std::path::PathBuf]) -> Playlist;

    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;

    /// Advance and return the next image for `mode`, using `rng` for the
    /// random modes. Returns None iff the playlist is empty.
    /// - Sequential: images[cursor], cursor = (cursor+1) % len.
    /// - Shuffle: walk shuffle_order; when exhausted, reshuffle (no repeat
    ///   within a pass; first item of a fresh pass may equal the last shown).
    /// - PureRandom: uniform random index each call; repeats allowed.
    pub fn next(&mut self, mode: CycleMode, rng: &mut impl rand::Rng)
        -> Option<std::path::PathBuf>;

    /// Remove a path that failed to apply, then return the following next().
    /// Used when WallpaperSetter reports a corrupt/unreadable image.
    pub fn skip_and_next(&mut self, bad: &std::path::Path, mode: CycleMode,
        rng: &mut impl rand::Rng) -> Option<std::path::PathBuf>;
}
```

Injecting `rng: &mut impl Rng` makes the random modes deterministically
testable (seed a `StdRng`).

**Dependencies:** `core::model`.

**Tested (TDD, unit):**
- **Folder scan filtering:** a temp folder with `a.jpg, b.JPEG, c.png, d.bmp,
  e.webp, f.txt, sub/g.jpg` → only the supported extensions included
  (case-insensitive); decide and document whether scan recurses into
  subfolders — **default: non-recursive** (top-level only); `sub/g.jpg`
  excluded. (Recommended default — implementer may make it recursive if
  trivially cheaper, but document it.)
- **Sequential:** sorted-by-filename order; wraps around after the last.
- **Shuffle no-repeat invariant:** across one full pass of N items, all N
  distinct indices appear exactly once (the key invariant).
- **PureRandom:** every returned index is in range; with a fixed seed the
  sequence is reproducible; repeats are permitted.
- **Empty handling:** `build([])` and build over a missing folder →
  `is_empty()`, `next()` → `None`.
- **skip_and_next:** removes the bad path and returns a valid following image
  (or `None` when that empties the list).

### 5.4 `core::scheduler` — global tick + advance logic

**Responsibility:** own the global interval, the global cycle mode, the
pause/resume flag, and the per-monitor playlists; on each tick (and on manual
**Next**) advance **every** monitor and apply via the `WallpaperSetter` seam.
Pure logic — **no OS timer here**; the OS timer (Win32) lives in `app` and
calls `scheduler.tick()`.

```rust
pub struct Scheduler<W: WallpaperSetter, R: rand::Rng> {
    setter: W,
    rng: R,
    mode: CycleMode,
    fit: FitMode,
    paused: bool,
    /// Per-monitor (id, folders -> Playlist).
    monitors: Vec<(MonitorId, Playlist)>,
}

impl<W: WallpaperSetter, R: rand::Rng> Scheduler<W, R> {
    pub fn new(setter: W, rng: R, mode: CycleMode, fit: FitMode) -> Self;

    /// (Re)build playlists from the current config + live monitor list.
    /// Preserves nothing about cursors (fresh build) — see Data flow.
    pub fn rebuild(&mut self, assignments: &[(MonitorId, Vec<PathBuf>)]);

    pub fn set_mode(&mut self, mode: CycleMode);
    pub fn set_fit(&mut self, fit: FitMode);

    pub fn pause(&mut self);
    pub fn resume(&mut self);
    pub fn is_paused(&self) -> bool;

    /// Advance every monitor and apply. No-op while paused.
    /// On WallpaperSetter error for a monitor: if the error indicates a bad
    /// image, skip_and_next and retry once; otherwise skip that monitor this
    /// tick (keep its current wallpaper). Never panics on a single failure.
    /// Returns a per-monitor outcome list for logging.
    pub fn tick(&mut self) -> Vec<TickOutcome>;

    /// Same advance+apply as tick(), but ignores `paused` (manual Next).
    pub fn next_now(&mut self) -> Vec<TickOutcome>;
}

pub struct TickOutcome {
    pub monitor: MonitorId,
    pub result: TickResult,   // Applied(path) | SkippedEmpty | Failed(String)
}
```

**Dependencies:** `core::model`, `core::playlist`.

**Tested (TDD, unit) using a MOCK `WallpaperSetter`:**
- `tick()` calls `set` once per **non-empty** monitor with the expected next
  path; empty monitors produce `SkippedEmpty` and no `set` call.
- `paused` → `tick()` performs no `set` calls; `next_now()` still applies.
- A mock setter that returns a "bad image" error → scheduler calls
  `skip_and_next` and retries; a mock that returns a generic OS error → that
  monitor is `Failed` and others still get applied.
- `set_mode` switches the advance semantics on subsequent ticks.
- All monitors advance **on the same tick** (one `tick()` → one advance each).

### 5.5 `app::monitors` — live display enumeration (Windows)

**Responsibility:** `#[cfg(windows)]` implementation of `MonitorEnumerator`
using `IDesktopWallpaper`: `GetMonitorDevicePathCount` / `GetMonitorDevicePathAt`
for the stable `MonitorId`, and `GetMonitorRECT` for resolution → orientation.

```rust
#[cfg(windows)]
pub struct DesktopWallpaperMonitors { /* holds the IDesktopWallpaper COM ptr */ }

#[cfg(windows)]
impl MonitorEnumerator for DesktopWallpaperMonitors {
    fn enumerate(&self) -> Result<Vec<MonitorInfo>, AppError>;
}
```

**Dependencies:** `core::model`, `windows`.
**Tested:** manual-integration only (real monitors; cannot run headless). See
Manual verification checklist. Logic that derives `Orientation` from a RECT is
factored into a pure helper in `core::model` and unit-tested there.

### 5.6 `app::wallpaper` — set one image on one monitor (Windows)

**Responsibility:** `#[cfg(windows)]` implementation of `WallpaperSetter`:
`IDesktopWallpaper::SetPosition(fit)` mapped from `FitMode`, then
`SetWallpaper(monitorId, path)`. Map a file-not-found / unsupported-image OS
error into an `AppError::Os` variant distinguishable as "bad image" so the
scheduler can `skip_and_next` (recommended: a dedicated `AppError::BadImage`
variant; add it to the enum if cleaner).

`FitMode` → `DESKTOP_WALLPAPER_POSITION` mapping:
`Fill→DWPOS_FILL`, `Fit→DWPOS_FIT`, `Stretch→DWPOS_STRETCH`,
`Center→DWPOS_CENTER`, `Tile→DWPOS_TILE`.

```rust
#[cfg(windows)]
pub struct DesktopWallpaperSetter { /* IDesktopWallpaper COM ptr */ }

#[cfg(windows)]
impl WallpaperSetter for DesktopWallpaperSetter {
    fn set(&self, monitor: &MonitorId, image: &Path, fit: FitMode)
        -> Result<(), AppError>;
}
```

`SetPosition` is global to the desktop, so set it before/with the first
`SetWallpaper`; re-applying it per call is acceptable and cheap.

**Dependencies:** `core::model`, `windows`. **Tested:** manual-integration.

### 5.7 `app::autostart` — HKCU Run registry (Windows)

**Responsibility:** `#[cfg(windows)]` implementation of `AutostartManager`.
Value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value name
`wallpaper-shuffler`, data = the quoted absolute path to the current `.exe`
(`std::env::current_exe`). `set_enabled(true)` writes it; `false` deletes it;
`is_enabled` checks presence.

```rust
#[cfg(windows)]
pub struct RunKeyAutostart;
#[cfg(windows)]
impl AutostartManager for RunKeyAutostart { /* ... */ }
```

**Dependencies:** `core::model`, `windows` (or `winreg`). **Tested:** manual.

### 5.8 `app::tray` — tray icon + context menu (Windows)

**Responsibility:** NWG tray icon and context menu with exactly four items:
**Next**, **Pause/Resume** (label toggles with state), **Settings**, **Exit**.
Each item dispatches to the app controller (calls `next_now`, toggle
pause/resume, open settings window, post quit).

**Dependencies:** `native-windows-gui`, app controller. **Tested:** manual.

### 5.9 `app::settings_ui` — settings window (Windows)

**Responsibility:** NWG window that:
- lists detected monitors, each with its **orientation label** and a flag if
  its folder list is empty/unconfigured (Error-handling table);
- lets the user assign one or more folders per monitor (folder picker; add /
  remove);
- edits the **global** interval (minutes, min 1) and **global** cycle mode;
- edits the **global** fit mode;
- toggles autostart (drives `AutostartManager`).

On apply: mutate `Config` → `config::save` → trigger a scheduler `rebuild` of
affected monitors. Closing/hiding the window keeps the app running in the tray.

**Dependencies:** `native-windows-gui`, `core::config`, app controller.
**Tested:** manual.

### 5.10 `app::main` — wiring + Win32 message loop

**Responsibility:** process startup, COM init (`CoInitializeEx`), build the real
seam implementations, load config, enumerate monitors, build the `Scheduler`,
start a Win32 timer (`SetTimer`) at `interval_secs`, run the message loop, and
handle **`WM_DISPLAYCHANGE`** (monitor hotplug) by re-enumerating + rebuilding
playlists while preserving assignments by `MonitorId`. Logs to
`%APPDATA%\wallpaper-shuffler\log.txt`. Single-instance is **not required**
(out of scope) unless trivial.

**Dependencies:** everything. **Tested:** manual.

---

## 6. Config schema & example

Structs are defined in §5.2. Example `config.toml`:

```toml
# %APPDATA%\wallpaper-shuffler\config.toml
interval_secs = 1800        # 30 min; minimum enforced is 60
cycle_mode = "Sequential"   # "Sequential" | "Shuffle" | "PureRandom"
fit_mode = "Fill"           # "Fill" | "Fit" | "Stretch" | "Center" | "Tile"
autostart = false

# Per-monitor folder assignments, keyed by the stable device-path id.
[monitors."\\\\?\\DISPLAY#DELA1B2#5&abc123&0&UID4352#{...}"]
folders = ["C:\\Wallpapers\\Landscape", "C:\\Wallpapers\\Landscape-extra"]

[monitors."\\\\?\\DISPLAY#GSM5678#5&def456&0&UID4353#{...}"]
folders = ["C:\\Wallpapers\\Portrait"]
```

Enum serde representation: default serde (variant name as a string, e.g.
`"Sequential"`). Keep it; it is human-editable and stable.

---

## 7. Data flow & lifecycle

**Startup:**
1. `CoInitializeEx`; create real seam impls.
2. `config::load_or_default(appdata_path)`. If reset → open settings.
3. `monitors.enumerate()` → live `MonitorInfo`s.
4. For each live monitor, look up its `MonitorConfig.folders` by `MonitorId`;
   `Playlist::build(folders)`. Unknown live monitors → empty playlist
   (unconfigured).
5. Build `Scheduler` (mode/fit from config); create tray; `SetTimer(interval)`.
6. Optionally apply an initial wallpaper set immediately (recommended: yes —
   call `next_now()` once at startup so screens populate without waiting a full
   interval).

**Tick (timer fires) / manual Next:**
- Timer → `scheduler.tick()`; tray **Next** → `scheduler.next_now()`.
- Each advances every monitor's playlist (per the global mode) and applies via
  `WallpaperSetter`. Bad-image errors → `skip_and_next` + retry; other failures
  → skip that monitor, keep current, log.

**Settings change:**
- UI mutates `Config` → `config::save` → if interval changed, `KillTimer` +
  `SetTimer(new)`; if mode/fit changed, `scheduler.set_mode/set_fit`; if folders
  changed, `scheduler.rebuild(affected assignments)`. Autostart toggle →
  `AutostartManager::set_enabled`.

**Display change (`WM_DISPLAYCHANGE`):**
- Re-enumerate monitors; rebuild playlists, preserving assignments by
  `MonitorId`. A newly attached monitor with no saved assignment starts
  **unconfigured** (empty playlist) until assigned in settings.

**Shutdown (tray Exit):**
- `KillTimer`, hide tray, `config::save` if dirty, `CoUninitialize`,
  `PostQuitMessage`.

---

## 8. Error-handling table

| Condition | Handling |
|---|---|
| Empty / missing assigned folder | Skip that monitor (keep current wallpaper); flag the monitor as unconfigured in settings. `Playlist::is_empty()` → `TickResult::SkippedEmpty`. |
| Corrupt / unreadable image | `WallpaperSetter::set` returns bad-image error → `playlist.skip_and_next` and retry once; if still failing/empty, skip monitor this tick. |
| Monitor unplug / plug (`WM_DISPLAYCHANGE`) | Re-enumerate, rebuild playlists, preserve assignments by `MonitorId`; new monitor starts unconfigured. |
| COM / IO failure (set, enumerate, registry) | Log to `%APPDATA%\wallpaper-shuffler\log.txt`; keep running; never panic. |
| Corrupt / missing config | `load_or_default` returns defaults + `needs-settings=true`; write defaults; open settings. |
| Interval below minimum | `Config::clamped()` raises to `MIN_INTERVAL_SECS` (60). |

---

## 9. Testing strategy

**TDD unit-tested on Linux/CI (pure logic, no Windows APIs):**
- `core::config` — serde round-trip, defaults, clamp, corrupt/missing handling.
- `core::playlist` — mode semantics, **Shuffle no-repeat invariant**,
  folder-scan filtering (§image-formats, recursion default), empty handling,
  `skip_and_next`.
- `core::scheduler` — advance/apply logic via a **mock `WallpaperSetter`**,
  pause/resume, bad-image skip+retry, generic-failure isolation, mode switch.
- `core::model` — `Orientation` derivation helper.

The mock seam (in `core` test code):

```rust
struct MockSetter { calls: RefCell<Vec<(MonitorId, PathBuf, FitMode)>>,
                    fail: Option<AppError> }
impl WallpaperSetter for MockSetter {
    fn set(&self, m: &MonitorId, p: &Path, f: FitMode) -> Result<(), AppError> {
        self.calls.borrow_mut().push((m.clone(), p.into(), f));
        match &self.fail { Some(e) => Err(/* clone */), None => Ok(()) }
    }
}
```

Seed RNG with `rand::rngs::StdRng::seed_from_u64(..)` for deterministic
random-mode tests.

**Manual-integration (Windows-only; cannot run headless in CI):** `monitors`,
`wallpaper`, `autostart`, `tray`, `settings_ui`, `main` message loop +
`WM_DISPLAYCHANGE`. Verified on the real machine per the checklist.

Test command everywhere: `cargo test`. Size check: `cargo build --release`.

---

## 10. Implementation order / milestones

Build pure logic + tests first (runs on any platform), Windows integration last.

1. **M1 — workspace + core scaffolding.** Two-crate workspace (§4.1),
   §release-profile on the `app` crate, `core::model` types + traits. `cargo
   test` green (empty).
2. **M2 — config (TDD).** Implement `core::config` test-first: defaults,
   round-trip, clamp, corrupt/missing. `cargo test` green on Linux.
3. **M3 — playlist (TDD).** Implement `core::playlist` test-first: scan
   filtering, Sequential, Shuffle no-repeat, PureRandom, empty, skip_and_next.
4. **M4 — scheduler (TDD).** Implement `core::scheduler` test-first with the
   mock setter: advance/apply, pause/resume, error handling, mode switch. **End
   of CI-runnable work — all of M2–M4 pass via `cargo test` before touching
   Windows.**
5. **M5 — Windows seam impls.** `app::monitors`, `app::wallpaper`,
   `app::autostart` behind the traits (§trait-seam). Compile on Windows.
6. **M6 — tray + message loop + timer.** `app::tray`, `app::main` wiring, COM
   init, `SetTimer`, `WM_DISPLAYCHANGE`, logging. App runs from the tray and
   rotates.
7. **M7 — settings UI.** `app::settings_ui`: monitor list + orientation labels,
   per-monitor folders, global interval/mode/fit, autostart toggle; save +
   rebuild.
8. **M8 — size pass.** `cargo build --release`; confirm §release-profile;
   measure `.exe` size and RAM; trim features/deps if needed.

---

## 11. Manual verification checklist (Windows-only behaviors)

Run on the real Windows 11 machine after M5–M8:

- [ ] App launches to the system tray with an icon; no console window.
- [ ] Tray menu shows **Next**, **Pause/Resume**, **Settings**, **Exit**.
- [ ] On a multi-monitor setup with different orientations, each monitor gets a
      wallpaper from its own assigned folder, correctly oriented (by
      construction).
- [ ] **Next** advances all monitors immediately.
- [ ] **Pause** stops the timer-driven rotation; **Resume** restarts it; **Next**
      still works while paused.
- [ ] Each fit mode (Fill/Fit/Stretch/Center/Tile) visibly changes how images
      are placed.
- [ ] Sequential = sorted filename order; Shuffle = no repeat within a pass;
      Pure random = repeats possible.
- [ ] Interval setting respected; minimum clamps to 1 minute.
- [ ] Autostart toggle adds/removes the `HKCU\...\Run\wallpaper-shuffler` value
      pointing at the exe; app launches at login when enabled.
- [ ] Unplug a monitor / plug a new one → app re-enumerates; assignments
      preserved by device-path id; new monitor shows as unconfigured until
      assigned.
- [ ] Empty/missing folder → that monitor keeps its current wallpaper and is
      flagged unconfigured in settings.
- [ ] Corrupt image in a folder → skipped; rotation continues.
- [ ] Corrupt/missing config → defaults written, settings opens.
- [ ] Errors appear in `%APPDATA%\wallpaper-shuffler\log.txt`; app stays alive.
- [ ] `cargo build --release` produces a single self-contained `.exe`; record
      its size and idle RAM; confirm §release-profile is in effect.
```
