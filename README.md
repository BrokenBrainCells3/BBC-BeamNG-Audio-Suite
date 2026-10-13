# BrokenBrainCells BeamNG 3D Audio Simulation

A Tauri 2 desktop app (Rust backend + vanilla JS frontend) for **real-time 3D spatial audio processing and visualization**, driven by live BeamNG.drive UDP telemetry.

It captures PC audio, runs it through a 16-band EQ → 3D spatializer → peak limiter DSP chain on a max-priority audio thread, positions 6 virtual speakers from BeamNG vehicle/camera data, visualizes everything live in the UI, and can embed + stream an Opera/Chrome window into the app and out to BeamNG mods.

> **Status:** Windows 10/11 is the primary target (WGC capture, Win32 embed, Windows media session). Core audio + UDP paths are cross-platform via CPAL.

---

## Table of Contents

- [What it does](#what-it-does)
- [How it works (pipeline)](#how-it-works-pipeline)
- [Feature tour](#feature-tour)
  - [1. Audio capture & DSP](#1-audio-capture--dsp)
  - [2. 3D spatial audio (BeamNG-driven)](#2-3d-spatial-audio-beamng-driven)
  - [3. Frequency analysis & visualizer](#3-frequency-analysis--visualizer)
  - [4. Amp / Sub / Box panel](#4-amp--sub--box-panel)
  - [5. Session stats](#5-session-stats)
  - [6. Opera embed (Win32 SetParent + CDP)](#6-opera-embed-win32-setparent--cdp)
  - [7. Opera stream (WGC → JPEG snapshots)](#7-opera-stream-wgc--jpeg-snapshots)
  - [8. Media info (Now Playing)](#8-media-info-now-playing)
  - [9. Discord webhook](#9-discord-webhook)
  - [10. MCP input forwarding](#10-mcp-input-forwarding)
- [UI panels](#ui-panels)
- [Backend module reference](#backend-module-reference)
- [Tauri commands (JS ↔ Rust API)](#tauri-commands-js--rust-api)
- [Ports & protocols](#ports--protocols)
- [Configuration](#configuration)
- [BeamNG setup](#beamng-setup)
- [Prerequisites](#prerequisites)
- [Build & run](#build--run)
- [Project structure](#project-structure)
- [Threading & performance notes](#threading--performance-notes)
- [Troubleshooting](#troubleshooting)
- [Known limitations](#known-limitations)
- [License & author](#license--author)

---

## What it does

1. **Captures** system audio via CPAL on a dedicated real-time thread.
2. **Processes** it through a 16-band peaking-EQ bank, 3D spatializer (ITD, head shadowing, sub crossover, rear voicing), and peak limiter.
3. **Positions** 6 virtual speakers (FL / FR / RL / RR / SUB-L / SUB-R, plus tweeter feeds) using live BeamNG UDP telemetry - or manual offsets when UDP is off.
4. **Analyzes** the signal with an 800-point FFT (~60 Hz resolution at 48 kHz) and streams band levels to the frontend at 60 Hz for the visualizer.
5. **Displays** levels, waveform/spectrum, amp state, session stats, and Now Playing media in a zero-dependency vanilla-JS UI.
6. **Embeds** Opera/Chrome as a Win32 child window inside the Tauri window and **streams** that region as JPEG snapshots (WGC capture, GDI `PrintWindow` fallback) for consumption by BeamNG mods / external tools.

---

## How it works (pipeline)

```
┌─────────────┐    ┌──────────────┐    ┌───────────────┐    ┌──────────────┐    ┌────────┐
│ CPAL input  │───▶│ 16-band EQ   │───▶│ 3D spatializer│───▶│ Peak limiter │───▶│ Output │
│ (WASAPI/    │    │ (biquad      │    │ (ITD + head   │    │ (0.9 ceiling)│    │ device │
│  CoreAudio/ │    │  peaking,    │    │  shadow + sub │    │              │    │        │
│  ALSA)      │    │  ISO centers)│    │  crossover +  │    │              │    │        │
└─────────────┘    └──────────────┘    │  rear voicing)│    └──────────────┘    └────────┘
                                       └───────┬───────┘
                                               │ reads (lock-free atomics /
                                               │ double-buffered positions)
                                               ▼
                                       ┌───────────────┐    ┌──────────────┐
                                       │ BeamNG UDP    │───▶│ Speaker      │
                                       │ listener      │    │ positions +  │
                                       │ (telemetry,   │    │ telemetry    │
                                       │  camera,      │    │ state        │
                                       │  electrics)   │    │              │
                                       └───────────────┘    └──────────────┘

FFT tap (post-EQ, 800-pt) ──▶ band levels ──▶ 60 Hz throttled events ──▶ frontend visualizer
```

Key design points:

- The **audio callback never blocks**: gains are read with `try_lock` + fallback, FFT/events use lock-free atomics and throttled emitters.
- Output goes through a **passthrough ring buffer** (16 384 frames, pre-filled with ~50 ms silence) drained by a background resampler thread - this decouples capture rate from output rate and prevents clicks/starvation.
- Speaker positions use **double buffering** so the audio thread always reads a consistent snapshot even while UDP updates arrive.
- Filter coefficient changes (EQ, sub crossover) are **glided** (one-pole smoothing) to stay below the audible-click threshold.

---

## Feature tour

### 1. Audio capture & DSP

- **Capture:** CPAL default input device, F32 path; device sample rate / channel count auto-detected. Output device resolved separately (configurable, persisted).
- **EQ bank:** 16 peaking biquad filters at ISO 266 1/3-octave centers:

  | Band | Freq | Typical content |
  |------|------|-----------------|
  | 1 | 60 Hz | Sub-bass |
  | 2 | 150 Hz | Bass |
  | 3 | 300 Hz | Low mids |
  | 4 | 600 Hz | Mids |
  | 5 | 1 kHz | Upper mids |
  | 6 | 2.4 kHz | Presence |
  | 7 | 4 kHz | Brilliance |
  | 8 | 6 kHz | Air |
  | 9 | 8 kHz | High air |
  | 10 | 10 kHz | Ultra high |
  | 11 | 12 kHz | Sparkle |
  | 12 | 14 kHz | Shine |
  | 13 | 16 kHz | Ultra sparkle |
  | 14 | 18 kHz | Air band |
  | 15 | 20 kHz | Top of hearing |
  | 16 | - | Master volume (reserved) |

- **EQ presets:** named presets (`EqPreset { name, gains[16], left_gain, right_gain }`) saved to `config.json`, loadable from the EQ panel.
- **Resampling:** Rubato for capture→output rate conversion.
- **Decoding/playback:** Symphonia (MP3/AAC/OGG/WAV/FLAC/PCM) + Rodio sink for clips/stingers.
- **Limiter:** peak limiter with 0.9 ceiling to prevent clipping on the output stage.

### 2. 3D spatial audio (BeamNG-driven)

- **6 virtual speakers** with per-speaker XYZ offsets (meters, car-relative) and independent level + mute:

  | Speaker | Default offset [x, y, z] | Default level |
  |---------|--------------------------|---------------|
  | Front-left | [−0.9, −2.0, 0.5] | 0.25 |
  | Front-right | [+0.9, −2.0, 0.5] | 0.25 |
  | Rear-left | [−0.9, +2.0, 0.5] | 0.25 |
  | Rear-right | [+0.9, +2.0, 0.5] | 0.25 |
  | Sub-left | [−0.3, +1.5, 0.3] | 0.50 |
  | Sub-right | [+0.3, +1.5, 0.3] | 0.50 |

  Plus separate **tweeter feeds** with configurable frequency range (default low/high clamps 500 Hz–20 kHz) and **sub crossover** range (defaults ~20–90 Hz, smoothed).
- **Enhancement toggles** (all independently switchable from the UI):
  - **ITD** - interaural time delay for left/right localization.
  - **Head shadowing** - high-frequency attenuation for the far ear.
  - **Sub crossover** - routes lows to the sub feeds.
  - **Rear voicing** - distinct EQ/level treatment for rear speakers.
- **UDP mode:** when enabled, speaker positions/distances are computed from the live BeamNG UDP stream every packet (vehicle pos/quaternion + camera pos/forward). Manual sliders are bypassed - the UI says so explicitly.
- **Manual mode:** when UDP is off, positions come from the speaker-offset sliders, `set_speaker_positions` / `reset_speaker_positions` commands.
- **Telemetry surfaced to UI:** vehicle speed, engine RPM, throttle (via `get_3d_audio_state`), plus full camera packet (pos, quaternion, forward vectors, gear, turbo, temps, fuel, inputs) cached for background polling.

### 3. Frequency analysis & visualizer

- 800-point forward FFT (RustFFT), Hann-windowed, 60 Hz event throttle for smooth UI.
- `get_visualizer_bands` aggregates FFT bins into the 15 EQ bands for bar rendering.
- Frontend `3d-audio.js` + visualizer code renders bars/waveform; a **WebSocket toggle** switches the transport:
  - **Off (default):** HTTP polling at `http://0.0.0.0:5000/values`.
  - **On:** WebSocket push at `ws://0.0.0.0:5001` (~30 fps).
- Peak-hold, clipping indicator, per-band event queue with throttled logging (avoids log spam at audio rates).

### 4. Amp / Sub / Box panel

- Amplifier state machine (`amp_state.rs`) with **override support** (`set_amp_override` / `get_amp_overrides`) - the UI can force amp parameters for testing.
- Sub crossover cutoffs are smoothed in the audio thread via atomic glide (`AMP_SUB_LOW_SMOOTH` / `AMP_SUB_HIGH_SMOOTH`) so mod-driven changes don't click.
- VU-style meters driven by the same band levels as the visualizer.

### 5. Session stats

- `session_stats.rs` tracks per-session and lifetime stats: runtime, peak levels, clip counts, band energies.
- Persisted to disk (`save_to_file` / `load_from_file`); viewable in the Session Stats panel; resettable via `reset_lifetime_stats`.
- Session log file list exposed via `get_session_log_files`.

### 6. Opera embed (Win32 SetParent + CDP)

- `embed.rs` reparents an Opera/Chrome top-level window into the Tauri window via Win32 `SetParent`, then keeps it glued to its HTML section:
  - Frontend sends its rect (CSS px + viewport size + scale); Rust maps to screen coords and re-places the child **at capture fps while streaming**, so it tracks scrolling/resizing.
  - Early rects arriving before the child exists are stored as `pending` and applied on embed.
- `launch_opera_embed` / `launch_embedded_app` / `embed_launched_app` cover: launching Opera directly, launching arbitrary exes (with icon extraction via `get_exe_icon` for the picker UI), and docking an already-running app by PID.
- A **watchdog** thread monitors the child (still alive? still visible? parent still valid?) and re-docks or reports as needed.
- `cdp.rs` is a Chrome DevTools Protocol client (default port 9222) used for tab/window control alongside the native embed.
- Non-Windows builds return a friendly error for all embed commands.

### 7. Opera stream (WGC → JPEG snapshots)

- `stream.rs` captures **only the Opera box**: it captures the **parent Tauri window** via Windows Graphics Capture (GPU compositor surface) and crops exactly to the Opera rect - so the rest of the app can never leak into the stream.
- **Why the parent and not the child?** A reparented `WS_CHILD` has no own DWM surface; child-targeted capture misbehaved. The parent always has one.
- **Fallback:** GDI `PrintWindow` on the Opera child HWND when WGC can't start (screen-independent).
- **Output:** frames are JPEG-encoded (`jpeg-encoder`, default 60 fps / quality 70, configurable) and mirrored to **drop files inside the BeamNG mods folder**, which vehicle HTML textures poll via `local://` with a cache-buster (`?v=N`).
- **Why disk snapshots?** BeamNG 0.39's CEF texture webviews cancel `http://` and hold `ws://` forever - both proven broken - so `local://` file polling is the only channel that reaches an in-game HTML texture.
- Works while the app is covered/occluded/on another virtual desktop; a **true minimize has no DWM surface** (nothing on Windows can capture that), so keep the app unminimized while streaming.
- `start_opera_stream` / `stop_opera_stream` + `stream_status` diagnostics (capture path, frame size vs. box size - if frames ever exceed the box, something regressed).

### 8. Media info (Now Playing)

- `media_info.rs` uses Windows `Media_Control` APIs to read the system Now Playing session: title, artist, duration, status, thumbnail.
- Frontend hero tile shows thumbnail + controls (prev / play-pause / next) wired to the OS session.

### 9. Discord webhook

- Optional webhook URL stored in config (`set_discord_webhook_url` / `get_discord_webhook_url`); used for event notifications (e.g. stream/session events).

### 10. MCP input forwarding

- `mcp.rs` manages per-app MCP client connections (host/port/enabled + last error).
- When an app is embedded with MCP enabled, inputs can be forwarded to the embedded app's vehicle (`embedded_app_id` / `embedded_vehicle_id`), including `inject_input_mcp` from the UDP path.

---

## UI panels

Side-menu (`src/index.html` + `src/js/*.js`, styled by `src/css/*.css`):

| Menu item | Element | What it does |
|-----------|---------|--------------|
| Frequency Analysis | `#beamng-panel` | Band bars, Start/Stop PC Audio Capture, UDP-stream toggle, WS-visualizer toggle, 3D enhancement toggles, speaker sliders/mutes |
| Equalizer | `#eq-panel` | 16-band sliders + L/R gains, preset save/load |
| Settings | `#settings-panel` | Input/output device pickers, BeamNG mods folder, Discord webhook URL, config persistence |
| Logs | `#log-panel` | Throttled `debug_log` event stream from Rust |
| Session Stats | `#session-stats-panel` | Session + lifetime stats, reset buttons |
| Amp / Sub / Box | `#amp-panel` | Amp state, sub/tweeter volumes + crossover ranges, rattle controls, overrides |
| Opera Browser | `#browser-panel` | Launch/embed Opera or arbitrary app, rect sync, MCP settings |
| Opera Stream | `#stream-panel` | Start/stop stream, fps/quality, `stream_status` diagnostics |

Plus: hero **Now Playing** tile, user-name prompt (persisted), loading screen, toast notifications. The window is fixed at **1280×720, non-resizable** (`tauri.conf.json`) - the embed rect math depends on it.

---

## Backend module reference

| File | Responsibility |
|------|---------------|
| `main.rs` | Binary entry, calls lib run |
| `lib.rs` | App setup, shared state (`AudioState`, `SpatialAudioState`, `ConfigState`, `EmbedState`, …), audio thread spawn, event wiring |
| `audio_capture.rs` | CPAL device enum + input/output stream building |
| `audio_processing.rs` | EQ bank (biquads), spatial processor glue, peak limiter, FFT workspace, event throttle |
| `audio_buffers.rs` | Ring/passthrough buffers (`STREAM_COUNT`, 16 384-frame cushion) |
| `spatial_audio.rs` | Speaker offsets/levels/mutes, ITD/head-shadow/crossover/rear-voicing flags, double-buffered vehicle/camera state, telemetry |
| `beamng_udp.rs` | UDP listener, OutGauge-style packet parsing (456-byte expected), camera-data cache, packet/health counters, MCP input injection hook |
| `electrics.rs` | BeamNG electrics UDP listener (EQ bands + volume channels), start/stop/status |
| `visualizer.rs` | FFT→band aggregation for frontend |
| `session_stats.rs` | Session/lifetime stats + persistence + log-file listing |
| `amp_state.rs` | Amp model + UI overrides |
| `commands.rs` | All `#[tauri::command]` handlers for 3D audio, speakers, config, presets |
| `config.rs` | `AppConfig` load/save (`config.json` in app-data dir), `ConfigManager` lock-safe helper |
| `event_queue.rs` | Throttled logging + band-event queue (`should_log`, `log_throttled`, `emit_band_event`) |
| `media_info.rs` | Windows Now-Playing session reader |
| `discord_webhook.rs` | Webhook send + URL persistence |
| `cdp.rs` | Chrome DevTools Protocol client |
| `embed.rs` | Win32 embed/launch/dock/watchdog/rect placement |
| `mcp.rs` | MCP client registry + input forwarding |
| `stream.rs` | WGC/GDI capture → JPEG → mods-folder snapshots + status |
| `event_queue.rs` | (see above) |

---

## Tauri commands (JS ↔ Rust API)

Invoke from JS via `window.__TAURI__.core.invoke('cmd_name', args)` (see `src/js/tauri-bridge.js` for wrappers). Selected commands:

**3D audio**
- `set_3d_audio_enabled(enabled)` / `is_3d_audio_enabled` - master switch (enabling also starts the UDP listener).
- `set_3d_audio_udp_enabled(enabled)` / `is_3d_audio_udp_enabled` - UDP-driven positioning.
- `get_3d_audio_state` → `{ enabled, front_left, front_right, rear_left, rear_right, subwoofer_left, subwoofer_right, tweeter_left/right, tweeter_low/high_freq, sub_low/high_freq, rattle_volume/threshold, vehicle_speed, engine_rpm, throttle }`.
- `get_3d_audio_enhancements` / `set_3d_audio_enhancements({ itd_enabled, head_shadow_enabled, sub_crossover_enabled, rear_voicing_enabled })`.

**Speakers / filters**
- `set_speaker_positions({...})` / `get_speaker_positions` / `reset_speaker_positions` - 6× XYZ (18 floats).
- `set_speaker_mutes({...})` / `get_speaker_mutes` - 6 booleans.
- `set_subwoofer_volume(v)` / `set_tweeter_volume(v)` - 0.0–1.0 clamped.
- `set_tweeter_freq_range(low, high)` / `set_sub_freq_range(low, high)` - Hz, validated/clamped.
- `set_rattle_volume(v)` / `set_rattle_threshold(v)`.

**Config / presets / devices**
- `save_app_config` / `load_app_config` / `get_preset_names` / `save_eq_preset` / `load_eq_preset`.
- `set_beamng_mods_folder(folder?)` / `get_beamng_mods_folder`.
- `set_discord_webhook_url` / `get_discord_webhook_url`.
- Audio device + capture commands (start/stop capture, device lists - see `lib.rs`/`audio_capture.rs`).

**BeamNG / electrics**
- `get_udp_status` → `{ running, packets_received, last_packet_size, size_mismatch, ... }`.
- `get_latest_camera_data_command` → full camera packet or null.
- `start_electrics_udp_listener` / `stop_electrics_udp_listener` / `is_electrics_udp_enabled` / `get_electrics_values` / `get_electrics_udp_status`.

**Session / amp**
- `get_session_stats` / `get_lifetime_stats` / `reset_lifetime_stats` / `get_session_log_files`.
- `set_amp_override(key, value)` / `get_amp_overrides`.

**Embed / stream**
- `launch_opera_embed` / `launch_embedded_app` / `embed_launched_app` / `get_exe_icon`.
- `start_opera_stream` / `stop_opera_stream` (+ `stream_status` diagnostics).

**Events (Rust → JS):** `debug_log` (throttled strings), visualizer/band events at ≤60 Hz, media/telemetry updates.

---

## Ports & protocols

| Port / channel | Direction | Purpose |
|----------------|-----------|---------|
| BeamNG UDP → app (default `4444`-family; expected packet 456 B) | BeamNG → app | Vehicle/camera telemetry + speaker-position derivation |
| Electrics UDP (separate listener) | BeamNG → app | EQ bands + volume channels |
| `http://0.0.0.0:5000/values` | app → UI/tools | Visualizer values via HTTP polling (default transport) |
| `ws://0.0.0.0:5001` | app → UI | Visualizer values via WebSocket push (~30 fps, toggle) |
| CDP `9222` | app ↔ Opera/Chrome | Tab/window control alongside native embed |
| `local://` drop files in BeamNG mods folder | app → BeamNG texture | JPEG snapshots polled by in-game HTML (`?v=N` cache-buster) |

> Ports are defaults; check `config.rs` / the Settings panel if you've customized them.

---

## Configuration

- Stored as pretty JSON at the Tauri app-data dir (`…/com.audio.app.desktop/config.json`; resolved by `get_config_path`):

```jsonc
{
  "last_input_device": "Microphone (USB) | null",
  "last_output_device": "Speakers (Realtek) | null",
  "presets": {
    "My Preset": { "name": "My Preset", "gains": [16 floats…], "left_gain": 1.0, "right_gain": 1.0 }
  },
  "beamng_mods_folder": "C:/…/BeamNG.drive/mods | null",
  "discord_webhook_url": "https://discord.com/api/… | null"
}
```

- 3D-audio runtime state (enabled flags, speaker levels/offsets/mutes, crossover ranges, telemetry) lives in memory in `SpatialAudioState` and is queried via `get_3d_audio_state`; speaker geometry persists only if you save it through the UI/config path you wire up.
- `.gitignore` excludes editor dirs (`.vscode`), build output (`dist`, `target`-adjacent), logs, and local-only notes - see `.gitignore` in the repo root.

---

## BeamNG setup

1. **Vehicle telemetry (UDP):**
   - BeamNG → Options → Gameplay → UDP (OutGauge-compatible sender).
   - Target IP `127.0.0.1`, port matching the app's listener (default `4444`-family - verify in Settings/`beamng_udp.rs`).
   - Enable sending; drive - the app's UDP counters (`get_udp_status`) should start incrementing. `size_mismatch` + expected-vs-actual size hints at a version/packet-layout mismatch.
2. **Electrics channel:** enable the electrics sender if you want EQ/volume driven from the car; control via the electrics start/stop commands.
3. **In-game screen (Opera stream):** set `beamng_mods_folder` in Settings to your BeamNG mods dir, start the Opera stream, and point a vehicle HTML texture at the snapshot files with `?v=N` cache-busting.

---

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust toolchain | 1.75+ (`rustup update stable`) | MSVC target on Windows |
| Node.js | 20+ | Only needed for `@tauri-apps/cli` |
| Tauri CLI | v2 (`cargo install tauri-cli --version ^2`) | Or `npx tauri` after `npm install` |
| WebView2 | Evergreen | Preinstalled on Win10/11; Tauri bootstraps if missing |
| Windows 10 1903+ | - | Required for WGC capture + Win32 embed |
| BeamNG.drive | 0.39 tested | UDP sender; CEF texture caveats (see limitations) |
| Opera GX / Chrome | any CDP-capable | For embed + CDP control |

---

## Build & run

```bash
# 0. Clone
git clone https://github.com/BrokenBrainCells3/BrokenBrainCells-BeamNG-3D-audio-Simulation.git
cd BrokenBrainCells-BeamNG-3D-audio-Simulation

# 1. Frontend deps (only the Tauri CLI - the UI itself is dependency-free)
npm install

# 2. Dev loop (hot reload, Rust logs in terminal, web console in WebView)
cargo tauri dev
# or: npx tauri dev

# 3. Fast sanity checks without full build
cargo check -p brokenbraincells-beamng-dsp
cargo clippy -- -D warnings   # optional

# 4. Production build (MSI + portable exe under src-tauri/target/release/bundle/)
cargo tauri build
```

**Release profile** (`src-tauri/Cargo.toml`) is tuned for *build speed* over peak runtime:

```toml
[profile.release]
opt-level = 1
incremental = true
codegen-units = 16
lto = false
debug = false
```

Bump `opt-level = 3` + `lto = true` if you want max runtime DSP performance at the cost of slower builds.

---

## Project structure

```
.
├── README.md
├── package.json                  # only devDep: @tauri-apps/cli
├── src/                          # frontend (no npm runtime deps)
│   ├── index.html                # panels: beamng/eq/settings/log/stats/amp/browser/stream
│   ├── js/  3d-audio.js, ui.js, tauri-bridge.js, amp-panel.js,
│   │        chat-system.js, extra-apps.js, opera-embed.js, opera-stream.js,
│   │        session-stats.js, utils.js, window-aspect.js
│   ├── css/  base/theme/layout/components/panels/modals/logs/session-stats/chat.css
│   └── assets/  loading.png, tauri.svg, javascript.svg
└── src-tauri/
    ├── Cargo.toml / Cargo.lock / build.rs
    ├── tauri.conf.json           # 1280×720 fixed window, CSP, bundle, fs scope
    ├── capabilities/             # Tauri v2 permission sets
    ├── icons/  /  resources/
    └── src/  (see module reference above)
```

---

## Threading & performance notes

- Audio I/O runs on a **`ThreadPriority::Max`** thread; the callback does no allocation, no blocking locks (`try_lock` + sane fallback), no per-sample events.
- Visualizer/band events are **throttled** (60 Hz audio UI, ~30 fps stream/WS) via `EventThrottle` + `event_queue` so the frontend never drowns.
- FFT workspace is **preallocated** (`FftPlanner` once, reused buffers) - no per-callback setup.
- Resampling happens on a **background thread** draining the passthrough ring; capture and output rates are fully decoupled.
- Logging from audio-rate paths goes through `log_throttled` / `should_log` to keep the Logs panel readable.
- `codegen-units = 16`, no LTO: parallel fast builds; audio still comfortably real-time since the hot loop is small and allocation-free.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `Failed to get input config` / no devices | No default input, exclusive-mode WASAPI lock, or permission issue | Set a default input in Windows Sound settings; close apps holding exclusive mode; check Logs panel |
| Clicks/pops on start | Output starvation | Already mitigated with 50 ms pre-fill; if it persists, raise buffer / check CPU contention (keep Threads at Max priority, close heavy background apps) |
| `UDP … size_mismatch` | BeamNG version changed packet layout | Compare `UDP_EXPECTED_SIZE` (456) vs `last_packet_size`; update parser in `beamng_udp.rs` |
| UDP counters stay 0 | Wrong IP/port or sender disabled | Re-check BeamNG UDP target `127.0.0.1` + port; firewall rule for the app; `get_udp_status` |
| Embed: child not visible / wrong position | Rect sync race or DPI scale | The stream loop re-places at capture fps - start streaming; check `pending` rect + scale in logs |
| Stream: black frames | App minimized (no DWM surface) or WGC denied | Unminimize (occluded is fine); check `stream_status.mode` - `PrintWindow` fallback should engage |
| In-game texture never updates | Pointed at `http://`/`ws://` instead of `local://` | Use disk snapshots via `local://` + `?v=N`; set `beamng_mods_folder` correctly |
| `ws://…:5001` no data | Visualizer WS toggle off | Enable "Visualizer over WebSocket" in the Frequency Analysis panel |
| CDP connect fails | Opera not launched with remote debugging | Launch via the app's embed flow (passes `--remote-debugging-port=9222`) or start Opera manually with that flag |
| Build: `tauri` command not found | CLI not installed | `cargo install tauri-cli --version ^2` then `cargo tauri dev` |

---

## Known limitations

- **Windows-only** for embed (Win32 `SetParent`), stream capture (WGC/GDI), and media session. Other OSes get friendly errors from those commands.
- **Don't minimize while streaming** - minimized windows have no DWM surface; occlusion/virtual-desktop is fine.
- **BeamNG 0.39 CEF textures:** `http://` gets cancelled and `ws://` held forever inside texture webviews - use `local://` snapshots.
- **Fixed 1280×720 window** - embed rect math assumes it; resizing support would need rect/scale rework.
- Sub-rattle clip is **intentionally disabled** (caused crackle on heavy bass) - see commented block in `lib.rs`.

---

## License & author

Private / MIT at the owner's discretion - see repository settings.

**Author:** BrokenBrainCells3 - https://github.com/BrokenBrainCells3/BrokenBrainCells-BeamNG-3D-audio-Simulation
