# Changelog

This file records notable changes to Stremio Native relative to the initial source snapshot.

## Unreleased - 2026-07-16

### Highlights

- Reworked the desktop shell and all primary pages around a reusable Slint component system aligned with the official Stremio interaction model.
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

- Reconnected Stremio Core stream selection to the statically linked libmpv runtime and Slint's shared OpenGL render path.
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
- Compiles tracing out of release builds to minimize production profiling overhead.

### Reliability and diagnostics

- Added a synchronous panic-log fallback with captured backtraces for failures that occur before the non-blocking logger drains.
- Handles poisoned synchronization primitives through explicit recovery instead of panicking in the UI and playback bridges.
- Added navigation tests for invalid tabs, player/details back behavior, rapid metadata navigation, search history, forward history, and addon-route scoping.
- Added configuration migration tests and image-cache budget tests.
- Preserves the Windows GUI subsystem in release builds so launching the application does not open a console window.

### Dependencies and build

- Added `discord-rich-presence`, MiMalloc, and the Slint Winit 0.30 integration required for native media-key events.
- Uses the workspace Turso dependency with default features disabled, reducing the dependency graph and avoiding unwanted default integrations.
- Updated `Cargo.lock` to the dependency closure used by the current successful release build.
- Release build command: `cargo build --release --package stremio-native`.

### Known limitations

- The bundled static libmpv SDK and `playback-mpv/build.rs` currently support only `x86_64-pc-windows-msvc`.
- UI parity work still benefits from manual visual validation at multiple window sizes and DPI scales.
- Playback, subtitle/audio menu behavior, and real streaming should be smoke-tested with live media after each renderer or player change.
- The player buffering pulse timer is not yet gated by player-page visibility; the measured minimized CPU use is low, but a dedicated redraw trace is still recommended.
- Standalone Slint preview files, runtime preview JSON, and captured QA screenshots are development artifacts and are intentionally not part of the release-build commit.
