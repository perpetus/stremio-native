param(
    [string]$OutputDirectory = (Join-Path $env:TEMP "stremio-slint-debug"),
    [ValidateSet("All", "Classic", "Cinematic")]
    [string]$Style = "All",
    [switch]$KeepScreenshots
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$appFile = Join-Path $repoRoot "app\ui\app.slint"
$fixtureFile = Join-Path $PSScriptRoot "runtime-preview.json"
$viewer = (Get-Command slint-viewer -ErrorAction Stop).Source
$appSource = Get-Content $appFile -Raw
$supportsUiStyle = $appSource -match 'property\s*<\s*UiStyle\s*>\s*ui-style'

if ($Style -eq "Cinematic" -and -not $supportsUiStyle) {
    throw "Cinematic preview requested, but MainWindow does not yet expose UiStyle ui-style."
}

$styles = if ($Style -eq "All") {
    if ($supportsUiStyle) { @("Classic", "Cinematic") } else { @("Classic") }
} else { @($Style) }

$pageStates = @(
    [pscustomobject]@{ Page = -1; Name = "auth"; States = @("login", "register", "auth-loading", "auth-error") },
    [pscustomobject]@{ Page = 0; Name = "board"; States = @("loaded", "loading", "empty", "continue-only", "catalogs-only", "hero-fallback", "rail-focus-start", "rail-focus-middle", "rail-focus-end", "profile-menu") },
    [pscustomobject]@{ Page = 1; Name = "discover"; States = @("catalog", "loading", "empty", "movie-selected", "series-selected", "type-dropdown", "catalog-dropdown", "genre-dropdown") },
    [pscustomobject]@{ Page = 2; Name = "library"; States = @("loaded", "loading", "empty", "type-dropdown", "sort-dropdown") },
    [pscustomobject]@{ Page = 3; Name = "addons"; States = @("loaded", "loading", "empty", "addon-details", "addon-details-loading", "addon-details-error", "filter-dropdown", "type-dropdown", "add-addon") },
    [pscustomobject]@{ Page = 4; Name = "settings"; States = @("general", "interface", "player-settings", "streaming", "shortcuts", "diagnostics", "exporting") },
    [pscustomobject]@{ Page = 5; Name = "calendar"; States = @("loaded", "loading", "signed-out", "empty") },
    [pscustomobject]@{ Page = 6; Name = "search"; States = @("results", "loading", "empty", "idle") },
    [pscustomobject]@{ Page = 7; Name = "details"; States = @("details-loading", "movie-streams", "movie-no-streams", "movie-filter", "series-episodes", "series-season-menu", "series-streams", "series-stream-loading", "series-no-streams") },
    [pscustomobject]@{ Page = 8; Name = "player"; States = @("player-loading", "buffering", "paused", "playing", "skip-intro", "subtitles", "audio", "speed", "stats", "options", "context-menu", "episodes", "player-error", "controls-hidden") }
)

$checkFiles = @($appFile) + @(
    Get-ChildItem (Join-Path $repoRoot "app\ui\pages") -Filter "*.slint" |
        Sort-Object Name |
        ForEach-Object { $_.FullName }
) + @(
    Get-ChildItem (Join-Path $repoRoot "app\ui\cinematic") -Recurse -Filter "*.slint" |
        Where-Object { $_.Name -notin @("theme.slint", "types.slint") } |
        Sort-Object FullName |
        ForEach-Object { $_.FullName }
)

$diagnostics = [System.Collections.Generic.List[string]]::new()
foreach ($file in $checkFiles) {
    $output = @(& $viewer --check $file 2>&1)
    if ($LASTEXITCODE -ne 0 -or $output.Count -gt 0) {
        $diagnostics.Add("CHECK $file`n$($output -join "`n")")
    }
}

if ($diagnostics.Count -gt 0) {
    $diagnostics | ForEach-Object { Write-Error $_ }
    exit 1
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$stateFile = Join-Path $OutputDirectory "current-state.json"
$fixture = Get-Content $fixtureFile -Raw | ConvertFrom-Json
$fixture.'debug-menu-open' = $false
$rendered = 0

$fixtureJson = $fixture | ConvertTo-Json -Depth 100
$referencePaths = [regex]::Matches($fixtureJson, 'docs/ui/[^"\\]+(?:/[^"\\]+)*\.(?:png|jpe?g)') |
    ForEach-Object { $_.Value } | Sort-Object -Unique
Add-Type -AssemblyName System.Drawing
foreach ($relativePath in $referencePaths) {
    if ($relativePath -like '*intentionally-missing*') { continue }
    $assetPath = Join-Path $repoRoot ($relativePath -replace '/', '\')
    if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
        throw "Preview fixture image does not exist: $relativePath"
    }
    try {
        $decoded = [System.Drawing.Image]::FromFile($assetPath)
        $decoded.Dispose()
    } catch {
        throw "Preview fixture image is unreadable: $relativePath ($($_.Exception.Message))"
    }
}

foreach ($uiStyle in $styles) {
  if ($supportsUiStyle) { $fixture.'ui-style' = $uiStyle.ToLowerInvariant() }
  $styleOutputDirectory = Join-Path $OutputDirectory $uiStyle.ToLowerInvariant()
  New-Item -ItemType Directory -Path $styleOutputDirectory -Force | Out-Null
  foreach ($page in $pageStates) {
    foreach ($state in $page.States) {
        $fixture.'debug-preview-page' = $page.Page
        $fixture.'debug-preview-state' = $state
        $fixture.'profile-menu-open' = $page.Page -eq 0 -and $state -eq "profile-menu"

        $json = $fixture | ConvertTo-Json -Depth 100
        [System.IO.File]::WriteAllText(
            $stateFile,
            $json,
            [System.Text.UTF8Encoding]::new($false)
        )

        $screenshot = Join-Path $styleOutputDirectory ("{0:D2}-{1}-{2}.png" -f ($page.Page + 1), $page.Name, $state)
        $previousErrorAction = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = @(& $viewer --load-data $stateFile --screenshot $screenshot $appFile 2>&1)
        $viewerExitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorAction
        $meaningfulOutput = @($output | Where-Object {
            $_.ToString() -notmatch 'intentionally-missing\.jpg'
        })
        if ($viewerExitCode -ne 0 -or $meaningfulOutput.Count -gt 0) {
            $diagnostics.Add("RENDER $uiStyle/$($page.Name)/$state`n$($meaningfulOutput -join "`n")")
        } else {
            $rendered++
            if (-not $KeepScreenshots) {
                Remove-Item -LiteralPath $screenshot -Force
            }
        }
    }
  }
}

Remove-Item -LiteralPath $stateFile -Force -ErrorAction SilentlyContinue

if ($diagnostics.Count -gt 0) {
    $diagnostics | ForEach-Object { Write-Error $_ }
    Write-Host "rendered=$rendered failures=$($diagnostics.Count)"
    exit 1
}

Write-Host "checked=$($checkFiles.Count) styles=$($styles.Count) assets=$($referencePaths.Count - 1) rendered=$rendered diagnostics=0"
if ($KeepScreenshots) {
    Write-Host "screenshots=$OutputDirectory"
}
