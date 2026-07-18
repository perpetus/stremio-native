# 🎬 Stremio Rust - Ultra-Fast & Lightweight Native Stremio Desktop Client

<!-- SEO Meta Tags & Keywords -->
<!-- Keywords: Stremio alternative client, Stremio Rust desktop, fast Stremio player, lightweight Stremio app, Stremio web ui offline, Slint media player Rust, BitTorrent streaming player, local database media center, open source stream server -->
<meta name="description" content="Stremio Rust is an ultra-fast, lightweight, and modern desktop client for Stremio. Built with Rust and Slint UI, it features a custom, open-source stream server instead of the proprietary server.js." />

Welcome to **Stremio Rust** — the ultimate high-performance, battery-friendly, and lightweight desktop media center player designed to run the official Stremio experience native on your computer.

If you are looking for a fast Stremio desktop client that starts instantly, runs smoothly, and uses minimal system memory, Stremio Rust is the perfect alternative.

See the [detailed changelog](CHANGELOG.md) for the current build's implementation notes and known limitations.

---

## ✨ Features and Functionality

### 🖥️ Modern Stremio Web UI Alignment
* **Unified Navbar & Fixed Sidebar**: Experience a premium layout featuring a top header panel with a search bar and user profile controls, alongside a fixed vertical navigation sidebar that reveals labels on hover.
* **Discover Catalog Split Preview**: Browse movies and TV series catalogs without leaving your current page. Clicking a media card slides open a detailed metadata panel featuring blurred poster art backdrops, overview, casting list, and genre filters.
* **Advanced Series & Episode Picker**:
  - **Horizontal Seasons Row**: Switch seasons instantly using capsule-shaped navigation buttons.
  - **Real-Time Episode Search**: Filter series episodes on-the-fly with a built-in search box.
  - **Detailed Episode Cards**: Each row displays preview thumbnails, sequence numbers, localized release dates, and watched checkmarks.
  - **Dynamic Stream List Sheet**: Switches smoothly to the stream provider sheet, complete with a `← Back to Episodes` navigation link.

### ⚡ Rust-Powered Performance & Hardware Acceleration
* **Custom Open-Source Stream Server**: Unlike the official Stremio client which relies on a separate Node.js-based `server.js` backend, Stremio Rust embeds a custom, open-source **stream server** in-process. This eliminates the separate Node.js runtime and reduces process-management overhead.
* **Low CPU & Battery Usage**: Leveraging hardware-accelerated video decoding, this client utilizes your computer's GPU for video playback, keeping your CPU cool and extending your laptop's battery life.

#### Measured Idle Footprint (Windows x64)

The currently running native `1.0.0` release remains a single process, uses **358.6 MB** of private working set, and averaged **0.19% CPU** while idle. Its memory footprint is **455.8 MB (56.0%) lower** than the retained official Stremio baseline.

| Metric | Official Stremio baseline | Current native release |
| :--- | ---: | ---: |
| Processes | `10` | **`1`** |
| Private working set | `814.4 MB` | **`358.6 MB`** |
| CPU (5-second normalized average) | Not recorded | **`0.19%`** |
| Read/write I/O (5-second average) | Not recorded | **`0.00 / 0.00 MB/s`** |
| Threads | `190` | **`72`** |
| Handles | `4,872` | **`891`** |
| Loaded modules | `201` | **`108`** |

The native values were captured on July 18, 2026 from the already-running, settled release process without changing its state. CPU and I/O are five-second samples; the remaining values are point-in-time readings. The official WebView2 values are the corresponding settled baseline retained from the previous performance investigation, so this remains an observational comparison rather than a controlled laboratory benchmark.

### 📦 Secure Offline Database Cache (Turso & Limbo)
* **Local-First Database Storage**: Stores all settings, historical logs, and metadata inside a single local database file (`stremio.db`) using the native `turso` engine.
* **Memory-Based Image Loading**: Poster artwork and thumbnails are cached as database blobs and decoded asynchronously on background thread pools (using the Rust `image` crate), keeping your UI rendering at a locked $60\text{ FPS}$ without disk lag.
* **Privacy-Focused**: No cloud synchronizations, trackers, or telemetry. Your viewing history, settings, and logs remain 100% private and stored locally.

---

## 🚀 How to Build and Run the App

Windows x64 builds automatically download and verify the pinned optimized `libmpv-2.dll`; no media binaries are stored in this repository. Linux builds use the system's dynamic `libmpv` through `pkg-config`. The Windows runtime currently targets the `x86-64-v3` CPU baseline.

### 1. Prerequisites
On Windows, install the `x86_64-pc-windows-msvc` Rust toolchain from [rustup.rs](https://rustup.rs/) and the Visual Studio 2022 C++ build tools/Windows SDK. On Linux, install Rust, `pkg-config`, the libmpv development package, and the native GUI packages listed in the release workflow.

### 2. Launching the Media Center
1. Open your terminal or shell command prompt.
2. Navigate to the cloned repository directory:
   ```bash
   cd stremio-native
   ```
3. Build and run the optimized release on Windows:
   ```powershell
   cargo build --release --package stremio-native
   .\target\release\stremio-native.exe
   ```

   Or on Linux:
   ```bash
   cargo build --release --package stremio-native
   ./target/release/stremio-native
   ```

*All settings, log consoles, and image databases are stored in the local `./storage/` folder inside the project directory.*
