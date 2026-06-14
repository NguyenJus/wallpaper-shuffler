# wallpaper-shuffler

A tiny Rust system-tray app for **Windows 11** that rotates your desktop
wallpapers across multiple monitors — including monitors with different
orientations, each getting its own correctly-oriented image.

Built for the smallest possible binary and lowest RAM: a single self-contained
`.exe` with no runtime dependencies.

## Features

- **Per-monitor folders.** Assign one or more image folders to each monitor.
  Put portrait images in a portrait monitor's folder and landscape images in a
  landscape monitor's folder — orientation is handled by construction, no
  cropping or rotation.
- **Three cycle modes:** *Sequential* (sorted by filename), *Shuffle* (random,
  no repeat until the folder is exhausted), and *Pure random* (random each tick).
- **Global interval and mode.** One interval and one cycle mode for all
  monitors; every monitor advances together on the same tick.
- **System-tray controls:** Next, Pause/Resume, Settings, Exit.
- **Auto-start at login** (on by default, toggleable).
- **Handles monitor hot-plug** — assignments are preserved by device path.

Supported image formats: `jpg`, `jpeg`, `png`, `bmp`.

## Install

Download the latest `wallpaper-shuffler.exe` from the
[Releases](https://github.com/NguyenJus/wallpaper-shuffler/releases) page and run
it. It lives in the system tray; right-click the tray icon for controls.

## Usage

- **Settings** (tray menu) lists detected monitors with their orientation and
  lets you assign folders per monitor, set the global interval and cycle mode,
  choose a fit mode, and toggle auto-start.
- **Next** advances all monitors immediately. **Pause/Resume** stops and
  restarts the timed rotation (Next still works while paused).

Configuration is stored at `%APPDATA%\wallpaper-shuffler\config.toml`; logs at
`%APPDATA%\wallpaper-shuffler\log.txt`.

## Building from source

The project is a two-crate Cargo workspace:

- `crates/core` — pure logic (config, playlist, scheduler). Builds and tests on
  any OS.
- `crates/app` — the Windows binary. Windows-only.

```sh
# Run the pure-logic tests anywhere (Linux/macOS/Windows):
cargo test -p wallpaper-shuffler-core

# Build the release .exe (on Windows):
cargo build --release
# -> target/release/wallpaper-shuffler.exe
```

CI builds and tests on every push; tagged releases (`v*`) build the `.exe` and
attach it to a GitHub Release automatically.
