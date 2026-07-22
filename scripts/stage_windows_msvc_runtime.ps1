[CmdletBinding()]
param(
    [Parameter()]
    [string]$BuildRoot = (Join-Path $PSScriptRoot "..\target\release")
)

$ErrorActionPreference = "Stop"

function Get-VisualStudioInstallationPath {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        return $null
    }

    $installation = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($installation)) {
        return $null
    }
    return $installation.Trim()
}

function Find-CrtDirectory {
    param(
        [string[]]$SearchRoots
    )

    $candidates = foreach ($root in $SearchRoots) {
        if ([string]::IsNullOrWhiteSpace($root) -or -not (Test-Path -LiteralPath $root -PathType Container)) {
            continue
        }
        Get-ChildItem -LiteralPath $root -Directory -Recurse -Filter "Microsoft.VC143.CRT" |
            Where-Object { $_.Parent.Name -eq "x64" }
    }

    $selected = $candidates |
        Sort-Object -Property @{ Expression = {
            $runtime = Join-Path $_.FullName "vcruntime140.dll"
            if (Test-Path -LiteralPath $runtime -PathType Leaf) {
                return [version](Get-Item -LiteralPath $runtime).VersionInfo.FileVersion
            }
            return [version]"0.0"
        } } -Descending |
        Select-Object -First 1

    if ($null -eq $selected) {
        throw "Could not locate an x64 Microsoft.VC143.CRT redistributable directory. Install the Visual Studio 2022 C++ redistributables."
    }
    return $selected.FullName
}

function Find-Dumpbin {
    param(
        [string]$VisualStudioPath
    )

    if (-not [string]::IsNullOrWhiteSpace($env:VCToolsInstallDir)) {
        $fromTools = Join-Path $env:VCToolsInstallDir "bin\Hostx64\x64\dumpbin.exe"
        if (Test-Path -LiteralPath $fromTools -PathType Leaf) {
            return $fromTools
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($VisualStudioPath)) {
        $toolsRoot = Join-Path $VisualStudioPath "VC\Tools\MSVC"
        if (Test-Path -LiteralPath $toolsRoot -PathType Container) {
            $candidate = Get-ChildItem -LiteralPath $toolsRoot -Directory |
                Sort-Object -Property Name -Descending |
                ForEach-Object { Join-Path $_.FullName "bin\Hostx64\x64\dumpbin.exe" } |
                Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
                Select-Object -First 1
            if ($null -ne $candidate) {
                return $candidate
            }
        }
    }

    $command = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }
    throw "Could not locate dumpbin.exe. Install the Visual Studio 2022 C++ build tools."
}

$resolvedBuildRoot = [System.IO.Path]::GetFullPath($BuildRoot)
$application = Join-Path $resolvedBuildRoot "stremio-native.exe"
$mpv = Join-Path $resolvedBuildRoot "libmpv-2.dll"
foreach ($required in @($application, $mpv)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Missing release binary: $required"
    }
}

$visualStudioPath = Get-VisualStudioInstallationPath
$redistRoots = @()
if (-not [string]::IsNullOrWhiteSpace($env:VCToolsRedistDir)) {
    $redistRoots += $env:VCToolsRedistDir
}
if (-not [string]::IsNullOrWhiteSpace($visualStudioPath)) {
    $redistRoots += Join-Path $visualStudioPath "VC\Redist\MSVC"
}
$crtDirectory = Find-CrtDirectory -SearchRoots $redistRoots
$dumpbin = Find-Dumpbin -VisualStudioPath $visualStudioPath

$stageDirectory = Join-Path $resolvedBuildRoot "msvc-runtime"
New-Item -ItemType Directory -Path $stageDirectory -Force | Out-Null
Get-ChildItem -LiteralPath $stageDirectory -File -Filter "*.dll" -ErrorAction SilentlyContinue |
    Remove-Item -Force
Get-ChildItem -LiteralPath $stageDirectory -File -Filter "runtime-manifest.sha256" -ErrorAction SilentlyContinue |
    Remove-Item -Force

$runtimeDlls = Get-ChildItem -LiteralPath $crtDirectory -File -Filter "*.dll"
if ($runtimeDlls.Count -eq 0) {
    throw "No runtime DLLs were found in $crtDirectory"
}
foreach ($dll in $runtimeDlls) {
    Copy-Item -LiteralPath $dll.FullName -Destination $stageDirectory -Force
}

$stagedDlls = Get-ChildItem -LiteralPath $stageDirectory -File -Filter "*.dll"
$available = @{}
foreach ($dll in $stagedDlls) {
    $available[$dll.Name.ToLowerInvariant()] = $true
}

$requiredImports = @{}
foreach ($binary in @($application, $mpv) + $stagedDlls.FullName) {
    $output = & $dumpbin /nologo /dependents $binary 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin failed while inspecting $binary`n$output"
    }
    foreach ($line in $output) {
        if ($line -match '^\s*((?:vcruntime|msvcp|concrt|vccorlib)[^\s]*\.dll)\s*$') {
            $requiredImports[$Matches[1].ToLowerInvariant()] = $true
        }
    }
}

$missing = @($requiredImports.Keys | Where-Object { -not $available.ContainsKey($_) } | Sort-Object)
if ($missing.Count -gt 0) {
    throw "The app-local MSVC runtime is incomplete. Missing: $($missing -join ', ')"
}

$manifestPath = Join-Path $stageDirectory "runtime-manifest.sha256"
$manifest = $stagedDlls |
    Sort-Object -Property Name |
    ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $($_.Name)"
    }
Set-Content -LiteralPath $manifestPath -Value $manifest -Encoding ascii

Write-Host "Staged $($stagedDlls.Count) MSVC runtime DLLs from $crtDirectory"
Write-Host "Verified runtime imports with $dumpbin"
Write-Host "Wrote $manifestPath"
