# Stremio Rust implementation progress

Last updated: 2026-07-18

## Working agreement

- Implement against the official UI and behavior in `C:\Users\invin\Documents\code\stremio-web`.
- Reuse icons from `C:\Users\invin\Documents\code\stremio-icons` through local Slint assets.
- The user performs runtime and visual verification.
- Do not launch the application during implementation.
- Run formatting as needed, but defer compilation until all implementation work is finished.
- Final implementation verification is one release-profile workspace check; the user performs the runtime/build smoke test before commit.
- Ignore models and controls that only apply to a custom streaming server.

## Current diagnosis

### Client-first cold startup

The prior startup path did not enter Slint's event loop until Turso setup,
stream-server startup, every Core storage read, model construction, and MPV
initialization had completed. The app now creates, shows, and services its loading
window first. Icon lookup, tray/update setup, and all engine work yield to the first
paint; tray setup failure is non-fatal; `shell_ready_ms` records the pre-window cold
path. Database/configuration work runs from Slint's local async executor; server
startup and all independent storage reads run concurrently; non-critical database
maintenance is delayed beyond the first-frame window.

The first runtime smoke test exposed Turso's `unexpected row during execution`
error because the row-returning `PRAGMA journal_mode = WAL` had been included in
the no-row schema batch. WAL setup now uses the query path and consumes its
result; the remaining pragmas and schema stay batched, so the fix does not move
database work ahead of the responsive shell or add unnecessary schema calls.

### Native shell lifecycle

The tray source existed but was not part of the compiled application. It is now a
top-level Slint `SystemTrayIcon` with event-loop-queued open/settings/update actions,
log access, quit, localized labels, and update state. The exact stream-server icon is
used for tray/window/executable/installer resources, and Windows caption colors match
the official shell. Single-instance `stremio:`/`magnet:` forwarding queues commands
until Core is ready.

Discord RPC now owns connection state on its existing worker thread. A failed IPC
connection retains the latest desired activity and retries with exponential backoff
from 2 to 30 seconds. Media, pause, and resume activity transitions trigger a retry
when its deadline is due; reconnecting republishes the pending activity immediately.
Duplicate enable events share the same deadline, preventing startup/state bursts from
hammering Discord or adding work to the UI thread.

### Cached details lifecycle

The current runtime log confirms the repeat-visit failure at navigation revision 7:
the same title was opened again, but Core emitted no subsequent `MetaDetails` event
because the requested selection and its resource state were already ready. The UI
had unconditionally enabled its skeleton before dispatching that no-op load, so no
later projection existed to clear it.

All details entry points now use one cache-aware route helper. It navigates first,
then immediately projects the selected Core model when both its id and ready metadata
match the requested title; otherwise the genuine loading skeleton remains active for
the normal async event. This is constant-time over the small per-addon resource list,
does not clone the Core model, and avoids both redundant requests and timer-based
workarounds. Back navigation is outside the loaded-only branch and therefore remains
available for slow or failed loads.

### MPV distribution

The old static SDK made the repository large and could collide with Skia's native
symbols. Windows now downloads the pinned optimized x86-64-v3 DLL archive during the
Cargo build, verifies archive/runtime/import-library/license hashes, caches it under
`target`, and deploys the runtime beside the executable. Linux uses dynamic libmpv via
`pkg-config`; no PowerShell sync step or tracked MPV binary remains.

### Remote dependency compatibility

Stremio Core's watched-bitfield crate pinned `flate2` to `1.0.*`, while the pinned
stream-server lifecycle revision requires `flate2 1.1.x`. Cargo will not resolve
those conflicting SemVer-compatible constraints. The current upstream Core head
was forked to `perpetus/stremio-core`, the constraint was corrected to
`flate2 = "1"`, and this workspace pins the resulting remote commit. No local
dependency path is required.

The stream-server build script now honors
`STREAM_SERVER_SKIP_WINDOWS_RESOURCES` for embedded clients. Stremio Native sets
that flag through Cargo configuration, preventing the server's standalone
`VERSIONINFO`, icon, and DPI manifest from colliding with the GUI executable's
own resource table. Standalone stream-server builds retain their resources.

### MPV startup delay

The earlier log at `C:\Users\invin\Downloads\storage\logs\stremio.log` contained
application startup and shutdown but no stream-selection session. Before the
client-first startup change, it showed libmpv and the shared OpenGL context
initializing before playback was requested.

The current runtime log exposed a new lifecycle race: the shell was painted
before deferred MPV startup, so `install_renderer` ran after Slint's one-shot
`RenderingSetup` callback. Audio decoding continued, but no shared render context
was created and every render pass reported that its context was `None`. Renderer
installation now requests a redraw and creates the context once from the first
available `RenderingSetup` or `BeforeRendering` callback. Slint guarantees its
OpenGL context is current in both states. Failed/missing initialization is logged
once instead of on every frame, avoiding the observed high-frequency warning I/O.

The same log also identified Slint's shared context as OpenGL ES 2.0 and showed
FemtoVG receiving `GL_INVALID_ENUM` after MPV rendering. The state guard had queried
or changed ES3-only draw/read framebuffer, pixel-buffer, sampler, texture-swizzle,
rasterizer-discard, and sized-texture state. Shared rendering now derives capabilities
from the actual GL version and extensions, skips unsupported state, restores the ES2
framebuffer through its single binding target, allocates an unsized RGBA texture on
ES2, and reports an unknown internal format to MPV as its API permits. The existing
double-buffered texture path and supported VAO extension remain intact.

### Immediate player teardown

The current runtime log records Player Back at `21:42:57.949`, Core's
`PlayerStopped` event 14 ms later, but libmpv's actual stopped event only at
`21:43:30.524`. Navigation and Core teardown were already prompt; the serialized
MPV actor was unable to process its queued stop while a synchronous network-capable
command was outstanding. External subtitle loading is the most important case on
this startup path.

Network-backed `sub-add` now uses `mpv_command_async()`. The actor returns to its
command loop as soon as MPV accepts each subtitle request, so Stop cannot sit behind
subtitle network I/O. Stop and shutdown first request cancellation of every outstanding
subtitle command. `loadfile` deliberately remains synchronous because MPV documents
that it returns before actual file loading begins, preserving its guaranteed order
relative to Stop; the client API allows asynchronous and synchronous commands to be
reordered. Immediate queueing errors and delayed command-reply failures retain the
existing warning path without blocking Slint's thread.

The shutdown panic was independent of Player Back. Its complete backtrace points
to `AppSession::shutdown` calling `AppTray::hide` after Slint's event loop had
finalized the tray visibility property. Shutdown now drops the tray component by
ownership, which removes the native icon without re-entering the finalized property
system.

The previous playback path opened every stream at time zero, waited for `file-loaded`, and then performed an exact seek to the resume position. Audio could begin while that second, expensive seek delayed the first visible video frame. The UI also covered decoded video whenever MPV reported cache buffering.

Implemented corrections:

- Pass resume time as the per-file `start=` option in MPV's current `loadfile` API.
- Remove the post-`file-loaded` exact seek.
- Track a real MPV render-update frame separately from a forced initialization surface.
- Reveal video as soon as the first real decoded frame is rendered.
- Keep cache buffering independent from initial loading so buffering never replaces working video with artwork.
- Use the official transparent, pulsing Stremio buffering mark with progress fill.
- Log `load_elapsed_ms` and `load_to_first_frame_ms` for future runtime verification.

### Video orientation and aspect ratio

- On the Windows Slint borrowed-texture path, MPV's FBO is sampled in its produced orientation; `flip_y=0` is required. Enabling MPV's optional flip was confirmed by runtime screenshots to invert the picture.
- The Slint video element uses `contain`; MPV remains responsible for aspect-preserving letterboxing.
- A render-context initialization surface does not count as a decoded video frame.

### Missing icons

Slint's SVG loader rejected the `currentColor` paint used by copied web icons. Local player/application SVG assets were normalized to a concrete white paint and remain tintable through Slint's `colorize` property.

### Persisted page hydration and first-launch catalogs

The persisted Turso database contains the expected authenticated profile and Continue Watching library records. The restart failure was caused after hydration: tab entry cloned a Core submodel on Tokio and queued it back to Slint, allowing that older snapshot to arrive after a newer Core projection. Tab entry and the initial active-tab projection now clone under the Core read lock, release it, and patch Slint immediately on its UI thread. This covers Board/Continue Watching, Discover, Library, Addons, Calendar, and Settings without adding a continuously running cache or background poll.

Core marks storage effects as sequential, but the desktop environment previously spawned both sequential and concurrent effects identically. A single FIFO worker now awaits sequential effects in submission order, preventing an older profile or library snapshot from overwriting a newer database value. A focused executor test records ordering even when the first submitted future yields.

The first-login addon gap came from Core's `ProfileChanged` behavior for `CatalogsWithExtra`: it rebuilds catalog request descriptors with unloaded content and intentionally does not start a range. The desktop event loop now detects unloaded pages introduced into the initial Board/Search range and dispatches only that bounded range after releasing the model lock. Persisted-profile startup still uses the existing initial range load, so it does not perform redundant full-catalog work.

Calendar metadata requests are cached by Core and do not automatically rebuild when the library or addon catalog sources change. Calendar entry now fingerprints only its relevant non-temporary library items plus addon catalogs, unloads the Calendar submodel only when that source changes, and otherwise reuses the ready schedule. Startup storage also uses Core's canonical constants, with a targeted legacy fallback for the historical `server_urls` key.

The official desktop interaction distinguishes browsing from navigation: a single Discover click selects and loads the side preview, while a double-click opens full details. Native Library keeps its direct single-click details route. The shared media card now exposes both signals, but only Discover consumes the double-click activation path.

## Implementation checklist

### Playback and player UI

- [x] Pinned optimized dynamic libmpv loading path with build-time download and hash verification.
- [x] MPV re-enabled and connected to Stremio Core stream selection.
- [x] Deferred MPV startup creates its shared OpenGL context from the first available render callback, including when the initial Slint setup callback has already passed.
- [x] Tokio/runtime boundary panic removed from MPV worker integration.
- [x] Turso invalid-value panic path handled separately from playback.
- [x] Correct OpenGL vertical orientation (`MPV_RENDER_PARAM_FLIP_Y = 0`).
- [x] Aspect-preserving video presentation.
- [x] Direct resume position in `loadfile`; no delayed exact seek.
- [x] First-real-frame tracking and timing instrumentation.
- [x] Official-style transparent buffering indicator.
- [x] Cache buffering no longer masks decoded video.
- [x] Network-backed MPV subtitle commands no longer block Player Back teardown.
- [x] Playback, seek, volume, mute, fullscreen, track, speed, and episode callbacks wired.
- [x] Seek and volume sliders use explicit pointer-drag behavior and root-anchored progress layers.
- [x] Top back/title/fullscreen geometry moved toward official web layout.
- [ ] Finish exact control-bar spacing and responsive collision handling.
- [ ] Finish exact subtitle menu parity.
- [ ] Finish exact audio menu parity.
- [ ] Confirm progress/buffer semantics are consistent (normalized playback vs percent buffered).

### Board and navigation

- [x] Official navigation colors and right-side icon placement substantially aligned.
- [x] Sidebar assets replaced with Stremio icon sources.
- [x] Header Stremio mark and sidebar icons share the exact navigation-rail center line.
- [x] `See All` restored to catalog headers.
- [x] Continue Watching kept visible while catalog sections refresh.
- [x] Make Continue Watching projection deterministic across profile/model update races.
- [ ] Verify correct video id, progress, poster, order, and dismiss action against core's preview model.
- [ ] Finish smooth wheel scrolling and horizontal pointer drag for catalog rows.
- [x] Ensure Calendar has isolated loading state, source-aware cache invalidation, and no stale tab-snapshot overwrite.

### Discovery, details, and search

- [x] Discovery data-model loading and poster cache integration added.
- [x] Discovery preview/details panel added.
- [x] Details background, metadata, stream-provider filter, and actions added.
- [x] Dedicated global search model, route, suggestions, and result page added.
- [x] Details stream-row play action remains vertically centered when descriptions wrap.
- [x] Repeated details visits immediately reuse and project a matching ready Core cache entry.
- [x] Details Back navigation remains available while metadata is loading.
- [x] Discover uses single-click preview and double-click details, while Library opens details with one click.
- [x] First-login addon-provider Movie/Series catalogs load their bounded initial Board/Search range without an application restart.
- [ ] Complete 1:1 discovery spacing, filters, poster grid, and right preview panel.
- [ ] Complete 1:1 details metadata, bottom actions, provider selector, and stream rows.
- [ ] Complete 1:1 search page layout and empty/loading states.

### Addons

- [x] Addon models include type labels and configuration flags.
- [x] Installed/community and type filters added.
- [x] Add-addon and addon-details dialogs added.
- [ ] Anchor configure/uninstall/install/share controls to the official fixed right action column.
- [ ] Wire Share addon behavior and ensure every action has its official icon.
- [ ] Correct multi-type filtering and exact header/search geometry.

### Release and documentation

- [x] Release builds use the Windows GUI subsystem so no console window is spawned.
- [x] Player parity plan exists at `docs/player-control-ui-parity-plan.md`.
- [x] This durable implementation ledger was created for context compaction.
- [x] Windows CI produces the executable, verified MPV runtime/licenses, installer, and updater archive from a clean checkout.
- [x] Linux CI installs system libmpv development metadata and builds the portable release path.
- [x] Linux MPV artifact hashing is compatible with `sha2` 0.11 without digest-type formatting assumptions.
- [x] Windows CI mirrors stream-server's pinned optimized static libtorrent 2.0.13 vcpkg baseline, overlay, triplet, and GitHub Actions cache instead of relying on runner-local libraries.
- [x] All GitHub Actions references use their current stable major lines; checkout and artifact upload are on v7.
- [x] A pushed `v*` tag automatically waits for Windows and Linux builds, collects updater-compatible assets, writes SHA-256 checksums, and publishes a release with direct downloads, version-matched changelog notes, categorized commit links, and a full comparison link.
- [x] Stremio Core and stream-server dependencies use pinned remote Git revisions for clean CI/CD checkouts.
- [x] Update the implementation ledger, rendering reference, MPV guide, README, and changelog.
- [x] Run `cargo fmt --all`.
- [x] Run the single final `cargo check --workspace --release` and fix all reported release-only errors (completed successfully on 2026-07-18).

## Key files

- `playback-mpv/src/actor.rs`: MPV command/event state, direct resume, buffering state.
- `playback-mpv/src/render.rs`: shared OpenGL texture rendering and first-frame signal.
- `app/src/mpv_integration.rs`: Slint/MPV bridge, control callbacks, renderer lifecycle.
- `app/ui/pages/player.slint`: video composition, buffering, controls, track menus.
- `app/src/event_loop.rs`: coalesced core-to-Slint model projection.
- `app/src/models/board.rs`: Continue Watching and Board catalogs.
- `app/src/models/calendar.rs`: isolated Calendar navigation and projection.
- `app/src/models/search.rs`: local suggestions and global catalog search.
- `app/ui/pages/discover.slint`: official-style discovery grid and preview.
- `app/ui/pages/details.slint`: metadata and streams detail route.
- `app/ui/pages/addons.slint`: addon list, filters, actions, and dialogs.

## Final handoff evidence to collect

- [x] `cargo check --workspace --release` completed successfully.
- [x] Cargo resolved the pinned Core fork commit `4562b37be8ea788801e12c608a6aa3e43c646123` and stream-server commit `73a23325a49e76f7dee0afe08bf3e1a9b3ef3eec`, which contains the lifecycle fix plus the embedded-resource opt-out.
- [x] `cargo build --package stremio-native` completed through the Windows debug linker without `CVT1100`; the resulting EXE reports Stremio `1.0.0` metadata.
- [x] The deployed `libmpv-2.dll` hash is `ade5cac46cfc397a3d5cd356a968cda7acf0debffb705a16509dafdf93029f5e`; the cached import library hash is `bef1b89f534bc86b33135e1f04fa2d5064b9d48b5de8bc9866665bbf43def793`.
- [x] Corrected the runtime Turso startup failure by consuming the row returned by `PRAGMA journal_mode = WAL` before executing the no-row schema batch.
- [x] Corrected audio-only playback caused by deferred MPV startup missing Slint's one-time `RenderingSetup` callback; explicit redraw and one-time `BeforeRendering` initialization now preserve both fast shell startup and video output.
- [x] Discord RPC retains pending activity and reconnects on a bounded background backoff, including retries during playback activity changes.
- [x] Confirmed from the current runtime log that a repeated same-id details route received no new Core state event; every route now reprojects matching ready cached state synchronously.
- [x] Removed OpenGL ES 3-only state operations from Slint's OpenGL ES 2 shared-render path while retaining extension-backed VAO state and double-buffered textures.
- [x] Moved network-backed `sub-add` to libmpv's asynchronous command API and cancel it before Stop while retaining ordered synchronous `loadfile` semantics.
- [x] Removed the post-event-loop `AppTray::hide` write responsible for the captured Slint constant-property panic.
- [x] `cargo fmt --all -- --check` and `git diff --check` pass for the targeted Turso and deferred-renderer corrections.
- [x] `cargo check --package stremio-native --bin stremio-native` completed successfully after both runtime corrections.
- [x] Final combined validation passed after the cache, GLES2, immediate Player Back, tray-shutdown, and logo-alignment corrections: `slint-viewer --check .\\app\\ui\\app.slint`, formatting, diff hygiene, and `cargo check --workspace --release` all completed successfully on 2026-07-18.
- [x] One combined validation passed on 2026-07-18 for persisted hydration, FIFO storage, and first-login catalogs: Slint syntax, formatting, diff hygiene, the focused FIFO executor test, and `cargo check --workspace --release` all succeeded.
- [x] Validated the corrected Discover single-click-preview/double-click-details callback path while retaining Library's one-click details behavior: Slint syntax, formatting, diff hygiene, and the release package check passed on 2026-07-18.
- [ ] Confirm the corrected Linux and Windows release jobs pass after the CI dependency patch; failed run `29631460575` identified the `sha2` 0.11 digest-format incompatibility and missing Windows vcpkg toolchain.
- [ ] Restart twice and confirm Continue Watching and Calendar project current data immediately; runtime verification remains with the user.
- [ ] Confirm addon-provider Movie/Series Board cards populate on the first authenticated launch, Discover previews on one click and opens details on double-click, and Library opens details with one click; runtime verification remains with the user.
- [ ] Rebuild and restart, then confirm repeat details visits leave the skeleton immediately and Back remains visible during loading; runtime verification remains with the user.
- [ ] Confirm the shared MPV texture replaces the player artwork with decoded video and the FemtoVG invalid-enum warnings no longer recur; runtime verification remains with the user.
- [ ] Confirm Player Back silences and unloads MPV immediately during both initial loading and active playback; runtime verification remains with the user.
- [ ] Confirm application quit removes the tray icon without a `Constant property being changed` panic; runtime verification remains with the user.
- No runtime launch by the implementation agent.
- A concise list of files changed and any remaining items that require the user's visual/runtime verification.
