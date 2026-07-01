# Alloy

Alloy is a lightweight Rust desktop music player built around a small core API and a modular app shell. It uses `egui`/`eframe` for the native UI and `rodio` for local audio playback, so it stays far below a webview/Tauri footprint while remaining cross-platform.

## What Works

- Native desktop UI with library, search, transport controls, seeking, volume, drag-and-drop, native file/folder pickers, and a polished default theme.
- Local music source module that scans folders or individual files.
- End-to-end local playback through `rodio`, including MP3, FLAC, Ogg/Vorbis, WAV, AAC/M4A where the backend supports it.
- FLAC and Ogg/Opus Vorbis-comment metadata for title, artist, album, and ReplayGain tags.
- Modular audio processing chain. The built-in ReplayGain and EQ processors are normal chain nodes, and native community processors can be ordered around them.
- Shuffle, repeat-all, repeat-one, restart/previous transport behavior, and a bit-perfect mode that bypasses Alloy DSP and app volume.
- JPEG/PNG sidecar album covers from files such as `cover.jpg`, `folder.png`, `front.jpg`, or an image matching the track file stem.
- Built-in themes: Graphite, Linen, and Signal.
- Optional Last.fm module for now-playing updates and scrobbling.
- Optional Discord Rich Presence module.
- Community module discovery for TOML manifest modules, external theme files, and native ABI audio processors that can be toggled into the playback chain.
- In-app settings window for configuring `alloy.toml`, modules, integrations, themes, library folders, album covers, and the audio chain.

## Build And Run

On Linux, install the native audio build dependencies first:

```sh
sudo apt-get install pkg-config libasound2-dev
```

Then build or run with the Rust tools in your cargo bin directory:

```sh
/home/max/.cargo/bin/cargo run --release --bin alloy
```

You can pass a music folder or file directly:

```sh
/home/max/.cargo/bin/cargo run --release --bin alloy -- /path/to/Music
/home/max/.cargo/bin/cargo run --release --bin alloy -- --library /path/to/Music
```

Inside the app, add folders from the native picker in the left panel or Settings, type a path manually, or drop audio files/folders onto the window.

## Platform Support

Alloy is written against cross-platform crates: `eframe`/`egui` for the native window and `rodio`/`cpal` for audio output. The code is intended to run on Linux, macOS, and Windows.

Current verification status:

- Linux: built and tested in this workspace.
- macOS: native GitHub Actions builds are configured for Intel and Apple Silicon runners.
- Windows: native GitHub Actions builds are configured for x86_64 MSVC.

This repository includes `.github/workflows/desktop-builds.yml`, which builds:

- `alloy-linux-x86_64`
- `alloy-windows-x86_64`
- `alloy-macos-x86_64` as `Alloy.app`
- `alloy-macos-aarch64` as `Alloy.app`

The local workspace here is Linux-only, so Windows and macOS artifacts should be produced by that workflow or by building on those operating systems directly.

Windows release builds are configured as GUI binaries so launching `alloy.exe` does not open a separate command prompt.

## Configuration

Alloy creates a config folder using the platform standard app config location. On Linux this is:

```text
~/.config/alloy/
```

The main file is `alloy.toml`. Built-in modules are independent toggles:

```toml
[modules."alloy.sources.local"]
enabled = true

[modules."alloy.audio.eq"]
enabled = true

[modules."alloy.audio.replaygain"]
enabled = true

[modules."alloy.integrations.lastfm"]
enabled = false

[modules."alloy.integrations.discord"]
enabled = false
```

You can edit this file from inside Alloy: open **Settings**, then use the **Config** tab to reload, edit, and apply raw TOML.

The audio pipeline is ordered by `audio_chain`:

```toml
[[audio_chain]]
id = "alloy.audio.replaygain"
enabled = true

[[audio_chain]]
id = "alloy.audio.eq"
enabled = true
```

Native audio processor modules can be added to the same list by module id. The Settings window also provides a timeline-style chain editor with enable toggles, ordering controls, and per-node configuration.

ReplayGain is configured independently from the chain node:

```toml
[replay_gain]
enabled = true
mode = "track"
preamp_db = 0.0
prevent_clipping = true
```

Playback behavior is configurable too:

```toml
[playback]
shuffle = false
repeat = "none"
bit_perfect = false
```

Bit-perfect mode is a best-effort Alloy bypass: the app disables its DSP chain and volume scaling. The final output can still be affected by the decoder, `rodio`/`cpal`, the selected output device format, or the operating-system mixer.

Album cover display is configurable:

```toml
[cover_art]
enabled = true
sidecar_images = true
```

For Last.fm, set these in `alloy.toml` or as environment variables before launching:

```sh
export LASTFM_API_KEY=...
export LASTFM_API_SECRET=...
export LASTFM_SESSION_KEY=...
```

For Discord RPC:

```sh
export DISCORD_CLIENT_ID=...
```

## Community Modules

Community modules live below the app config `modules` directory.

Manifest-only modules are TOML files that Alloy can list and toggle in the module browser:

```toml
id = "community.sources.example"
name = "Example Source"
version = "0.1.0"
category = "music-source"
description = "Example source module manifest."
authors = ["You"]
enabled_by_default = false
capabilities = ["network-source"]
```

External themes go in `modules/themes/*.toml`:

```toml
id = "midnight-green"
name = "Midnight Green"
mode = "dark"

[colors]
background = "#0c1110"
surface = "#121918"
surface-muted = "#1d2927"
panel = "#17211f"
text = "#f1f7f3"
muted-text = "#9fb0aa"
accent = "#7bd8a4"
accent-text = "#06120c"
hover = "#24332f"
danger = "#ef6f75"
```

Native audio processor plugins expose this symbol:

```rust
#[no_mangle]
pub extern "C" fn alloy_plugin_entry() -> *const alloy_core::NativePluginDescriptorV1 {
    // Return a stable descriptor with ABI_VERSION and manifest JSON.
    todo!()
}
```

The ABI structs live in `alloy-core`, which intentionally has a small dependency surface for plugin authors.

If a native plugin descriptor provides `create`, `process`, and `destroy` callbacks, Alloy creates an instance when playback starts and passes interleaved `f32` sample blocks through it at that processor's position in `audio_chain`.

## Verification

```sh
/home/max/.cargo/bin/cargo fmt --all
/home/max/.cargo/bin/cargo check --workspace
/home/max/.cargo/bin/cargo test --workspace
```
