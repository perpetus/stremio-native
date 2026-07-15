param(
    [switch]$CheckOnly,
    [switch]$ReuseSource,
    [switch]$PackageOnly,
    [ValidateSet("x86_64")]
    [string]$Architecture = "x86_64",
    [string]$VisualStudioPath
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$PSNativeCommandUseErrorActionPreference = $true
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$lockPath = Join-Path $repoRoot "mpv.lock.json"
$pinnedPath = Join-Path $repoRoot "playback-mpv\src\pinned.rs"
$distDir = Join-Path $repoRoot "dist\mpv\windows-$Architecture-static"
$workDir = Join-Path $repoRoot "target\mpv-static-build"
$headers = @("client.h", "render.h", "render_gl.h")
$githubHeaders = @{ "User-Agent" = "stremio-native-mpv-static-sync" }
if ($PackageOnly) {
    $ReuseSource = $true
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-ClientApi([string]$ClientHeader) {
    $line = Select-String -LiteralPath $ClientHeader -Pattern '^#define MPV_CLIENT_API_VERSION MPV_MAKE_VERSION\((\d+),\s*(\d+)\)' | Select-Object -First 1
    if (-not $line) {
        throw "MPV_CLIENT_API_VERSION was not found in $ClientHeader"
    }
    return @{
        major = [int]$line.Matches[0].Groups[1].Value
        minor = [int]$line.Matches[0].Groups[2].Value
    }
}

function Assert-WorkspacePath([string]$Path) {
    $root = [IO.Path]::GetFullPath($repoRoot).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    $resolved = [IO.Path]::GetFullPath($Path)
    if (-not $resolved.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside the repository: $resolved"
    }
}

function Invoke-Native([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program failed with exit code $LASTEXITCODE"
    }
}

$release = Invoke-RestMethod -Headers $githubHeaders "https://api.github.com/repos/mpv-player/mpv/releases/latest"
if ($release.prerelease -or $release.draft) {
    throw "GitHub releases/latest returned a non-stable release"
}
$tag = [string]$release.tag_name
$locked = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json

if ($CheckOnly -and $locked.release -ne $tag) {
    throw "MPV is stale: repository pins $($locked.release), latest stable is $tag. Run scripts/sync-mpv.ps1."
}

New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$sourceUrl = "https://github.com/mpv-player/mpv/archive/refs/tags/$tag.tar.gz"
$archive = Join-Path $workDir "$tag.tar.gz"
Invoke-WebRequest -Headers $githubHeaders $sourceUrl -OutFile $archive
$sourceSha = Get-Sha256 $archive

$headerHashes = [ordered]@{}
$headerPaths = @{}
foreach ($header in $headers) {
    $path = Join-Path $workDir $header
    Invoke-WebRequest -Headers $githubHeaders "https://raw.githubusercontent.com/mpv-player/mpv/$tag/include/mpv/$header" -OutFile $path
    $headerHashes[$header] = Get-Sha256 $path
    $headerPaths[$header] = $path
}
$api = Get-ClientApi $headerPaths["client.h"]

if ($CheckOnly) {
    if ($locked.source.url -ne $sourceUrl -or $locked.source.sha256 -ne $sourceSha) {
        throw "Pinned source metadata does not match the official $tag archive"
    }
    if ($locked.clientApi.major -ne $api.major -or $locked.clientApi.minor -ne $api.minor) {
        throw "Pinned client API does not match $tag headers"
    }
    foreach ($header in $headers) {
        if ($locked.headers.$header -ne $headerHashes[$header]) {
            throw "Pinned hash for $header does not match $tag"
        }
    }
    $pinned = Get-Content -LiteralPath $pinnedPath -Raw
    $expectedApi = "ApiVersion::new($($api.major), $($api.minor))"
    if (-not $pinned.Contains($expectedApi)) {
        throw "Generated Rust API pin does not match $tag headers"
    }
    if ($locked.linkage -and $locked.linkage -ne "static") {
        throw "The MPV lock describes $($locked.linkage) linkage; static linkage is required"
    }
    Write-Host "MPV $tag client API $($api.major).$($api.minor) is current and configured for static linkage."
    exit 0
}

$extractDir = Join-Path $workDir "source"
Assert-WorkspacePath $extractDir
if (-not $ReuseSource) {
    if (Test-Path $extractDir) {
        Remove-Item -LiteralPath $extractDir -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
    Invoke-Native "tar" @("-xf", $archive, "-C", $extractDir, "--strip-components=1")
}
elseif (-not (Test-Path (Join-Path $extractDir "ci\build-win32.ps1"))) {
    throw "-ReuseSource was requested, but no complete MPV source tree exists at $extractDir"
}

$python = (Get-Command python -ErrorAction Stop).Source
Invoke-Native $python @("-m", "pip", "install", "--disable-pip-version-check", "--upgrade", "meson")
$pythonScripts = & $python -c "import sysconfig; print(sysconfig.get_path('scripts'))"
if ($LASTEXITCODE -ne 0) {
    throw "Could not locate Python's scripts directory"
}
$env:PATH = "$pythonScripts;$env:PATH"

foreach ($program in @("meson", "ninja", "git", "nasm")) {
    if (-not (Get-Command $program -ErrorAction SilentlyContinue)) {
        throw "$program is required to build the static MPV SDK"
    }
}

if (-not $VisualStudioPath) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        throw "Visual Studio Installer's vswhere.exe was not found"
    }
    $VisualStudioPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if (-not $VisualStudioPath) {
    throw "Visual Studio with the x64 C++ toolchain was not found"
}
$devShell = Join-Path $VisualStudioPath "Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Import-Module $devShell
Enter-VsDevShell -VsInstallPath $VisualStudioPath -SkipAutomaticLocation -DevCmdArguments "-arch=x64 -host_arch=x64" | Out-Null

$llvmBin = Join-Path $VisualStudioPath "VC\Tools\Llvm\x64\bin"
if (Test-Path $llvmBin) {
    $env:PATH = "$llvmBin;$env:PATH"
}
$env:CC = "clang"
$env:CXX = "clang++"
$env:CC_LD = "lld-link"
$env:CXX_LD = "lld-link"
$env:RUST_LD = "lld-link"
$env:WINDRES = "llvm-rc"

# Git for Windows bundles Perl and other Unix build helpers used by libaom.
# They are not consistently added to PATH by package managers or CI images.
$gitExecPath = & git --exec-path
if ($LASTEXITCODE -ne 0) {
    throw "Could not determine the Git for Windows installation path"
}
$gitRoot = [IO.Path]::GetFullPath((Join-Path $gitExecPath "..\..\.."))
$gitUnixTools = Join-Path $gitRoot "usr\bin"
if ((Test-Path (Join-Path $gitUnixTools "perl.exe")) -and -not (Get-Command perl -ErrorAction SilentlyContinue)) {
    $env:PATH = "$gitUnixTools;$env:PATH"
}

$buildDir = Join-Path $extractDir "build"
Assert-WorkspacePath $buildDir
if (-not $PackageOnly -and (Test-Path $buildDir)) {
    Remove-Item -LiteralPath $buildDir -Recurse -Force
}

if (-not $PackageOnly) {
    Push-Location $extractDir
    try {
        Invoke-Native "meson" @("wrap", "update-db")
        foreach ($wrap in @("expat", "harfbuzz", "libpng", "zlib")) {
            if (-not (Test-Path (Join-Path "subprojects" "$wrap.wrap"))) {
                Invoke-Native "meson" @("wrap", "install", $wrap)
            }
        }

        # MPV's own Windows CI script is the source of truth for its static dependency
        # graph. We only change its final product from the CLI executable to a reusable
        # static SDK and disable Vulkan, which would otherwise remain a runtime DLL.
        $officialScript = Get-Content -LiteralPath "ci\build-win32.ps1" -Raw
        $officialScript = [regex]::Replace(
            $officialScript,
            '(?s)# Wrap shaderc.*?(?=# Manually wrap spirv-cross)',
            ''
        )
        $officialScript = $officialScript.Replace("-Dlibmpv=false", "-Dlibmpv=true")
        $officialScript = $officialScript.Replace("meson setup build", "meson setup build -Db_vscrt=mt")
        $officialScript = $officialScript.Replace("-Dtests=true", "-Dtests=false")
        $officialScript = $officialScript.Replace("-Dlibplacebo:shaderc=enabled", "-Dlibplacebo:shaderc=disabled")
        $officialScript = $officialScript.Replace("-Dlibplacebo:vulkan=enabled", "-Dlibplacebo:vulkan=disabled")
        $officialScript = $officialScript.Replace("-Dlibplacebo:d3d11=enabled", "-Dlibplacebo:d3d11=disabled")
        $officialScript = $officialScript.Replace("-Dshaderc=enabled", "-Dshaderc=disabled")
        $officialScript = $officialScript.Replace("-Dvulkan=enabled", "-Dvulkan=disabled")
        $officialScript = $officialScript.Replace("-Dffmpeg:vulkan=auto", "-Dffmpeg:vulkan=disabled")
        $officialScript = $officialScript.Replace("-Dd3d11=enabled", "-Dd3d11=disabled")
        $officialScript = $officialScript.Replace("-Dsubrandr=enabled", "-Dsubrandr=disabled")
        $officialScript = $officialScript.Replace("ninja -C build mpv.exe mpv.com", "ninja -d keeprsp -C build mpv.exe mpv.com")
        $officialScript = $officialScript.Replace("cp ./build/subprojects/vulkan-loader/vulkan.dll ./build/vulkan-1.dll", "")
        $localBuildScript = Join-Path $workDir "build-win32-static.ps1"
        Set-Content -LiteralPath $localBuildScript -Value $officialScript -Encoding utf8
        & $localBuildScript
        if ($LASTEXITCODE -ne 0) {
            throw "MPV's official Windows build script failed"
        }
    }
    finally {
        Pop-Location
    }
}

$mpvLibrary = Join-Path $buildDir "libmpv.a"
if (-not (Test-Path $mpvLibrary)) {
    throw "MPV did not produce the expected static library at $mpvLibrary"
}
$linkResponse = Get-ChildItem -LiteralPath $buildDir -Recurse -Filter "mpv.exe.rsp" | Select-Object -First 1
if (-not $linkResponse) {
    throw "Ninja did not retain MPV's linker response file; the static dependency graph cannot be packaged"
}

$staticInputs = [ordered]@{}
$wholeArchives = @{}
$systemLibraries = [ordered]@{}
$staticInputs[[IO.Path]::GetFullPath($mpvLibrary)] = $false
$tokens = [regex]::Matches((Get-Content -LiteralPath $linkResponse.FullName -Raw), '(?:[^\s"]+|"[^"]*")+')
foreach ($match in $tokens) {
    $token = $match.Value.Trim('"')
    $whole = $false
    if ($token.StartsWith("/WHOLEARCHIVE:", [StringComparison]::OrdinalIgnoreCase)) {
        $whole = $true
        $token = $token.Substring("/WHOLEARCHIVE:".Length).Trim('"')
    }
    if ($token.StartsWith("-l") -and $token.Length -gt 2) {
        $name = $token.Substring(2)
        $systemLibraries[$name.ToLowerInvariant()] = $name
        continue
    }
    if (-not ($token.EndsWith(".lib", [StringComparison]::OrdinalIgnoreCase) -or
        $token.EndsWith(".a", [StringComparison]::OrdinalIgnoreCase))) {
        continue
    }
    $candidate = if ([IO.Path]::IsPathRooted($token)) { $token } else { Join-Path $buildDir $token }
    if (Test-Path $candidate) {
        $resolved = [IO.Path]::GetFullPath($candidate)
        $staticInputs[$resolved] = $staticInputs.Contains($resolved) -and $staticInputs[$resolved] -or $whole
        if ($whole) {
            $wholeArchives[$resolved] = $true
        }
    }
    else {
        $name = [IO.Path]::GetFileNameWithoutExtension($token)
        if ($name) {
            $systemLibraries[$name.ToLowerInvariant()] = $name
        }
    }
}

Assert-WorkspacePath $distDir
if (Test-Path $distDir) {
    Remove-Item -LiteralPath $distDir -Recurse -Force
}
$libraryDir = Join-Path $distDir "lib"
New-Item -ItemType Directory -Force -Path $libraryDir | Out-Null

$linkLines = @(
    "# Generated by scripts/sync-mpv.ps1 from MPV's successful static link command."
    "# Format: static|archive, whole-static|archive, or system|library."
)
$sdkLibraries = @()
$dependencyIndex = 0
foreach ($entry in $staticInputs.GetEnumerator()) {
    $source = [string]$entry.Key
    # Meson folds these dependencies into libmpv.a with link_whole. Their GNU
    # archives are therefore redundant, and link.exe cannot read their long
    # member names even though LLVM's linker can.
    $sourceName = [IO.Path]::GetFileName($source)
    if ($sourceName.Equals("libdl.a", [StringComparison]::OrdinalIgnoreCase) -or
        $sourceName.Equals("libuchardet.a", [StringComparison]::OrdinalIgnoreCase)) {
        continue
    }
    if ([IO.Path]::GetFullPath($source) -eq [IO.Path]::GetFullPath($mpvLibrary)) {
        $linkName = "mpv"
    }
    else {
        $baseName = [regex]::Replace([IO.Path]::GetFileNameWithoutExtension($source), '[^A-Za-z0-9_]', '_')
        $linkName = "stremio_mpv_dep_{0:D3}_{1}" -f $dependencyIndex, $baseName
        $dependencyIndex++
    }
    $destination = Join-Path $libraryDir "$linkName.lib"
    Copy-Item -LiteralPath $source -Destination $destination
    $kind = if ($wholeArchives.ContainsKey([IO.Path]::GetFullPath($source))) { "whole-static" } else { "static" }
    $linkLines += "$kind|$linkName"
    $sdkLibraries += [ordered]@{
        name = $linkName
        source = [IO.Path]::GetFileName($source)
        wholeArchive = ($kind -eq "whole-static")
        sha256 = Get-Sha256 $destination
    }
}
foreach ($library in $systemLibraries.Values) {
    $linkLines += "system|$library"
}

$linkManifest = Join-Path $distDir "link-libraries.txt"
$linkLines | Set-Content -LiteralPath $linkManifest -Encoding utf8
Copy-Item -LiteralPath (Join-Path $extractDir "LICENSE.GPL") -Destination $distDir
Copy-Item -LiteralPath (Join-Path $extractDir "LICENSE.LGPL") -Destination $distDir

$sdkManifest = [ordered]@{
    schemaVersion = 1
    target = "x86_64-pc-windows-msvc"
    mpv = $tag
    clientApi = [ordered]@{ major = $api.major; minor = $api.minor }
    staticLibraries = $sdkLibraries
    systemLibraries = @($systemLibraries.Values)
}
$sdkManifestPath = Join-Path $distDir "static-sdk.json"
$sdkManifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $sdkManifestPath -Encoding utf8

$lock = [ordered]@{
    schemaVersion = 2
    channel = "stable"
    linkage = "static"
    release = $tag
    clientApi = [ordered]@{ major = $api.major; minor = $api.minor }
    source = [ordered]@{ url = $sourceUrl; sha256 = $sourceSha }
    headers = $headerHashes
    artifacts = [ordered]@{
        "windows-$Architecture-static" = [ordered]@{
            directory = "dist/mpv/windows-$Architecture-static"
            library = [ordered]@{ file = "lib/mpv.lib"; sha256 = Get-Sha256 (Join-Path $libraryDir "mpv.lib") }
            linkManifest = [ordered]@{ file = "link-libraries.txt"; sha256 = Get-Sha256 $linkManifest }
            sdkManifest = [ordered]@{ file = "static-sdk.json"; sha256 = Get-Sha256 $sdkManifestPath }
        }
    }
    verifiedAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd")
}
$lock | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $lockPath -Encoding utf8
@"
// This file is updated by scripts/sync-mpv.ps1 from the pinned MPV headers.
pub const HEADER_CLIENT_API_VERSION: ApiVersion = ApiVersion::new($($api.major), $($api.minor));
"@ | Set-Content -LiteralPath $pinnedPath -Encoding utf8

Write-Host "Built and packaged statically linked MPV $tag client API $($api.major).$($api.minor) into $distDir"
