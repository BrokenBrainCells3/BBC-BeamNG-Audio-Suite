# BrokenBrainCells — BeamNG DSP & Audio Suite

A Tauri v2 desktop application for **BeamNG.drive** that provides real-time audio processing, spatial/3D audio, voice communications, a 16-band parametric equalizer, session statistics, and an in-game audio visualizer via an HTTP bridge to BeamNG Lua scripts.

---

## Features

- **Frequency Analysis** — Live FFT-based spectrum visualization (sub, bass, mid, treble bands) from any audio input device
- **16-Band Parametric Equalizer** — Full EQ with presets, applied in real time to the audio passthrough
- **3D / Spatial Audio** — Positions virtual speakers in 3D space using BeamNG vehicle and camera telemetry received over UDP
- **Voice Communications** — WebSocket voice chat with Opus encoding/decoding via a companion Python server
- **BeamNG UDP Telemetry** — Reads live vehicle data (speed, RPM, electrics) from BeamNG's OutGauge/UDP interface
- **Audio Visualizer Bridge** — Built-in HTTP server (port 5000) exposes band values as JSON so BeamNG Lua scripts can drive in-game visual effects at 60 Hz
- **Local Music Player** — Play MP3, AAC, OGG, WAV, and FLAC files directly in the app with full EQ applied
- **Session Statistics** — Track and review per-session audio and driving data
- **Featured Mods Browser** — Browse and link to curated BeamNG mods from within the app
- **Discord Webhook** — Optional integration to post session events or stats to a Discord channel

---

---

## Prerequisites

Install the following before building:

### 1. Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```
Verify: `rustc --version`

### 2. Node.js & npm
Download from [nodejs.org](https://nodejs.org/) (v18 or later recommended).  
Verify: `node --version` and `npm --version`

### 3. Tauri CLI prerequisites (Windows)
- **Microsoft C++ Build Tools** — install via [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload
- **WebView2** — already included in Windows 10/11; otherwise install from [Microsoft](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)


---

## Project Structure

```
audio_app/
├── src/                        # Frontend (HTML, CSS, JS)
│   ├── index.html
│   ├── css/
│   └── js/
├── src-tauri/                  # Rust / Tauri backend
│   ├── src/
│   │   ├── lib.rs              # App entry, audio thread
│   │   ├── audio_capture.rs
│   │   ├── audio_processing.rs
│   │   ├── spatial_audio.rs
│   │   ├── beamng_udp.rs
│   │   ├── session_stats.rs
│   │   └── ...
│   ├── Cargo.toml
│   └── tauri.conf.json
```

---

## Building the App

### 1. Install JavaScript dependencies

```bash
cd audio_app
npm install
```

### 2. Development build (with hot reload)

```bash
npm run tauri dev
```

This compiles the Rust backend and opens the app window. The frontend reloads automatically when you edit files in `src/`.

### 3. Production build

```bash
npm run tauri build
```

The compiled installer and executable are output to:

```
src-tauri/target/release/bundle/
```

On Windows you will find an NSIS installer (`.exe`) and optionally an MSI inside that folder.




---

## BeamNG Integration


### UDP Telemetry (3D Audio & Session Stats)

Enable **OutGauge** in BeamNG (`Settings → Other → OutGauge`) and point it at `127.0.0.1`. The app listens on the default OutGauge port and uses vehicle/camera position data to drive the spatial audio engine.

---

## Configuration

Settings are persisted automatically by the app. Key options available in the UI:

- Input audio device selection
- Output audio device selection
- EQ band gains and preset management
- Speaker positions for 3D audio (FL, FR, RL, RR, Sub L, Sub R)
- Discord webhook URL
- Voice server connection details

---

