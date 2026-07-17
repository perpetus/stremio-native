# Runtime UI simulation

The simulation runs the real `MainWindow` and its production page components.
It does not maintain a second copy of page layout code.

Open the interactive harness from the repository root:

```powershell
slint-viewer --load-data app/ui/debug/runtime-preview.json app/ui/app.slint
```

Use the settings button in the lower-left corner to choose a page and one of
its states. The fixture includes deterministic cards, episodes, streams,
calendar entries, addon data, player tracks, and playback diagnostics.
Cinematic preview media is loaded from `docs/ui`; run commands from the
repository root so those paths resolve correctly.

Compile every page and render every simulated state headlessly:

```powershell
powershell -ExecutionPolicy Bypass -File app/ui/debug/check-runtime-states.ps1
```

Keep the generated screenshots for visual comparison (Classic and Cinematic
are written to separate subdirectories):

```powershell
powershell -ExecutionPolicy Bypass -File app/ui/debug/check-runtime-states.ps1 -KeepScreenshots
```

Render only the Cinematic state matrix:

```powershell
powershell -ExecutionPolicy Bypass -File app/ui/debug/check-runtime-states.ps1 -Style Cinematic -KeepScreenshots
```

`-Style All` is the default and renders Classic and Cinematic into separate subdirectories. Before
rendering, the script verifies that each expected `docs/ui` fixture exists and
can be decoded. The one `intentionally-missing.jpg` reference is exempt because
it exercises the production missing-artwork fallback.

The `docs/ui` media paths exist only in the JSON debug fixture. Production
Slint and Rust sources must remain free of `docs/ui` references, so release
builds continue to use live metadata and the application image cache.

The harness is opt-in. `MainWindow.debug-preview-enabled` remains `false` in
normal application startup; the JSON fixture enables it only for
`slint-viewer`.
