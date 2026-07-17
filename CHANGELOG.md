# Changelog

This file records notable changes to Stremio Native relative to the initial source snapshot.

## Unreleased - 2026-07-17

### Highlights

- Reworked the desktop shell and all primary pages around a reusable Slint component system aligned with the official Stremio interaction model.
- Implemented the Windows-first rendering architecture: Skia OpenGL by default, process-level FemtoVG/software fallback, and selectable native-window, shared-OpenGL, or software MPV video output.
- Restored end-to-end libmpv playback with direct OpenGL texture composition, full player controls, deterministic first-frame handling, and event coalescing.
- Added typed navigation, global search, Discord Rich Presence, TheIntroDB skip segments, centralized Turso storage, and Windows media-key handling.
- Reduced retained image memory and redundant model/UI work; the latest settled native build measured 406.7 MB in Task Manager versus the retained 814.4 MB official Stremio baseline.

### Application shell and navigation

- Added a typed `NavigationController` with explicit routes for tabs, search, metadata details, addon details, and the player.
- Added back and forward history, route revisions, discover-preview selection, and stale-request rejection so late asynchronous responses cannot overwrite newer navigation.
- Centralized projection of active tab, details, addon dialog, and player visibility into `MainWindow`.
- Reworked the sidebar and top navigation with official Stremio assets, expandable labels, search, profile controls, and window actions.
- Added global keyboard shortcuts plus Windows media-key handling for play, pause, and next episode.
- Added pause-on-occlusion behavior when the corresponding player preference is enabled.

### Slint UI and design system

- Expanded the theme from a small palette into semantic tokens for backgrounds, modal and drawer surfaces, controls, overlays, dividers, scrims, status colors, focus, title bar, muted text, and skeleton states.
- Added reusable action groups, buttons, checkboxes, radio buttons, number and color inputs, text/search inputs, selects, sliders, overlays, horizontal navigation and scrolling, shortcuts, transitions, fallbacks, feedback states, loading placeholders, media carousels, metadata rows, metadata previews, and share prompts.
- Rebuilt Board, Discover, Library, Calendar, Addons, Details, Search, Auth, Settings, and Player pages to use the shared component system.
- Added loading, empty, error, placeholder, context-menu, modal, bottom-sheet, and drawer states across the main routes.
- Added Continue Watching projection alongside board catalogs without hiding it during catalog refreshes.
- Added Discover split-preview navigation, metadata actions, genre/catalog filters, and grid presentation.
- Added global search suggestions and a dedicated results route.
- Added addon source/type filters, installed/community grouping, add-addon flow, details, configuration state, and install/uninstall actions.
- Added calendar item projection and isolated calendar navigation into metadata details.

### Playback and libmpv

- Added `--renderer=auto|skia-opengl|femtovg|software` and `--video-output=auto|native-window|shared-opengl|software`, including split argument forms, legacy `SLINT_BACKEND` precedence, combination validation, and a hidden relaunch-attempt argument.
- Added a real one-pixel renderer probe with a five-second timeout. OpenGL attempts require Slint's native OpenGL API and inspect framebuffer alpha; software attempts require an actual Winit redraw.
- Added process-level automatic fallback from Skia OpenGL to FemtoVG OpenGL and then Slint software. Forced renderers receive one attempt, and retry processes append their diagnostics to the original renderer log.
- Added an unowned, non-activating Win32 video host that remains directly behind the transparent Slint client area, tracks physical client geometry and z-order, and hides while the player is closed, minimized, invisible, or occluded.
- Added native MPV D3D11 output through `wid` with a strict `gpu-next,gpu,direct3d` VO list, D3D11 GPU context/API, WARP fallback, direct D3D11VA decoding, and direct rendering where supported.
- Replaced the static MPV archive closure with the checksum-pinned optimized x86-64-v3 `libmpv-2.dll` from shinchiro's MPV-listed Windows builds, eliminating the Skia/SPIRV-Cross duplicate-symbol link failure while retaining D3D11, libplacebo, shaderc, and SPIRV-Cross support.
- Retained shared OpenGL as an explicit compatibility path with copy-safe hardware decoding, and report `--video-output=shared-opengl` when native VO configuration fails.
- Added a dedicated MPV software-render worker with a 1280x720 aspect-preserving cap, 100 ms resize debounce, 64-byte buffer/stride alignment, opaque RGB0-to-RGBA normalization, a newest-frame-only mailbox, hidden-surface skipping, and deterministic shutdown.
- Added a typed Slint player surface mode. Native playback makes the player scene transparent and omits the video image; shared-OpenGL and software playback continue to render an image below Slint controls.
- Made first-frame handling output-specific: native video waits for `PlaybackRestarted` with a configured VO and video track, while OpenGL and software modes wait for an actual rendered frame.
- Observe and log `vo-configured`, `current-vo`, `gpu-context`, `hwdec-current`, video-track presence, and MPV's numeric end error code.
- Reconnected Stremio Core stream selection to the pinned libmpv runtime and Slint's shared OpenGL render path.
- Passes resume position directly through MPV's per-file `start=` option, removing the delayed second exact seek after `file-loaded`.
- Distinguishes render-context initialization from a real decoded frame and reveals video only after the first actual MPV render update.
- Keeps cache buffering separate from initial loading so decoded video is not covered by artwork during later buffering.
- Corrected Windows borrowed-texture orientation with `MPV_RENDER_PARAM_FLIP_Y = 0` and retained aspect-preserving presentation.
- Added a coalescing playback-event inbox that replaces adjacent high-frequency state snapshots without reordering control events.
- Added a UI projection cache and scheduler so only changed playback properties are sent to Slint and at most one state update is queued at a time.
- Added playback, pause, seek, short-seek, volume, mute, fullscreen, speed, audio-track, subtitle-track, subtitle-language, episode, scale, and stream callbacks.
- Added buffered progress, playback statistics, track metadata, stream metadata, player error state, episode drawer, and auto-hiding controls.
- Added first-frame and load timing diagnostics for future playback profiling.
- Added tests for playback event coalescing and orderly inbox shutdown.

### Discord and skip segments

- Added an isolated Discord Rich Presence worker with connect, disconnect, set activity, clear activity, artwork, and playback timestamp support.
- Added configurable Discord activity projection from the current media and playback state.
- Added TheIntroDB v3 segment retrieval for intros, recaps, credits, and previews, with request timeouts and optional bearer authentication.
- Added configurable segment types and context-sensitive skip buttons in the player.
- Added boundary tests for active skip-segment selection.

### Storage and configuration

- Centralized application and Stremio Core storage on a shared Turso database installed through `core-env`.
- Added WAL, normal synchronous mode, memory temp storage, and a bounded SQLite page cache for the local database.
- Added `core_storage`, settings, and logs schemas plus batch setting reads/writes and log pruning.
- Added migration of legacy JSON storage buckets into Turso and migration of `config.json` into the database with a `.bak` handoff.
- Removed the obsolete database-backed image BLOB cache in favor of the bounded memory/filesystem image pipeline.
- Added versioned application configuration and migration of the generated legacy palette while preserving user-customized themes.
- Added configuration for TheIntroDB credentials and per-segment visibility.
- Merges both `library_recent` and `library` storage buckets during startup to preserve recent and long-term library state.

### Performance and resource use

- Registered MiMalloc as the global allocator.
- Reduced the base decoded-image cache from 256 MiB to 32 MiB and added a separate required-image working set with a 60-second idle expiry.
- Added bounded image-fetch workers with independent network, disk-read, and decode concurrency limits.
- Batches image refreshes onto the Slint event loop and safely rearms refresh delivery when new URLs arrive during a pending update.
- Added stable fingerprints for catalogs, profiles, details, calendar, search, addons, and stream lists to skip unchanged model projections.
- Patches cached images into existing Slint models rather than rebuilding entire page models for every image completion.
- Coalesces high-frequency MPV state and redraw work before it reaches the UI thread.
- Avoids rewriting generated icon fonts and Slint font imports when their contents have not changed.
- Loads independent storage buckets concurrently and keeps blocking stream-server startup off the asynchronous executor.
- Added scoped profiling modes for UI, I/O, playback, and full traces in debug builds.
- Retains INFO-level file tracing in release builds so renderer fallback and native-video failures remain diagnosable; debug-only Chrome tracing is still opt-in.

### Reliability and diagnostics

- Added a synchronous panic-log fallback with captured backtraces for failures that occur before the non-blocking logger drains.
- Handles poisoned synchronization primitives through explicit recovery instead of panicking in the UI and playback bridges.
- Added navigation tests for invalid tabs, player/details back behavior, rapid metadata navigation, search history, forward history, and addon-route scoping.
- Added configuration migration tests and image-cache budget tests.
- Preserves the Windows GUI subsystem in release builds so launching the application does not open a console window.

### Dependencies and build

- Enabled Slint's explicit Winit, Skia OpenGL, FemtoVG, software, Winit 0.30, and raw-window-handle feature set while disabling implicit defaults.
- Added typed rendering errors, OpenGL capability inspection, and the target-gated Win32 APIs required by the native video host.
- Kept the static MSVC CRT ABI aligned across Rust, Skia, and the executable's native crates while isolating MPV's MinGW runtime and codec closure behind its DLL C API.
- Added `discord-rich-presence`, MiMalloc, and the Slint Winit 0.30 integration required for native media-key events.
- Uses the workspace Turso dependency with default features disabled, reducing the dependency graph and avoiding unwanted default integrations.
- Updated `Cargo.lock` to the dependency closure used by the current successful release build.
- Patched `skia-bindings` locally so its Cargo build script selects a complete Windows SDK and gives GN, Ninja, and clang-cl a temporary short source path when no static-CRT prebuilt archive exists.
- Made `playback-mpv/build.rs` consume and validate the tracked runtime manifest, link the COFF import library, and deploy `libmpv-2.dll` beside debug executables and tests.
- Replaced the multi-hour MPV source-build sync path with a SHA-256-pinned prebuilt downloader and reproducible packager; ordinary Cargo builds remain offline and never invoke PowerShell.
- Debug builds now use ordinary Cargo with no PowerShell setup step: `cargo build --locked --package stremio-native`.

### Known limitations

- The bundled optimized libmpv DLL and `playback-mpv/build.rs` currently support only `x86_64-pc-windows-msvc` and require an x86-64-v3-capable processor.
- The native-window video host is Windows-only; other platforms resolve automatic video output to shared OpenGL until target-specific native hosts and libmpv packages are added.
- A clean rust-skia build compiles Skia from source because upstream does not publish this static-CRT feature combination; the local Cargo patch handles SDK selection and legacy source-path limits automatically.
- UI parity work still benefits from manual visual validation at multiple window sizes and DPI scales.
- Playback, subtitle/audio menu behavior, and real streaming should be smoke-tested with live media after each renderer or player change.
- The player buffering pulse timer is not yet gated by player-page visibility; the measured minimized CPU use is low, but a dedicated redraw trace is still recommended.
- Standalone Slint preview files, runtime preview JSON, and captured QA screenshots are development artifacts and are intentionally not part of the release-build commit.
