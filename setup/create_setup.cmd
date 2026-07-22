@echo off
setlocal

cd /d "%~dp0.."

if not defined APP_VERSION (
  for /f "tokens=3" %%V in ('findstr /b /c:"version = " app\Cargo.toml') do set "APP_VERSION=%%~V"
)

if not defined APP_VERSION (
  echo Unable to determine the application version from app\Cargo.toml.
  exit /b 1
)

if not defined ISCC_EXE set "ISCC_EXE=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if not exist "%ISCC_EXE%" set "ISCC_EXE=%ProgramFiles%\Inno Setup 6\ISCC.exe"

if not exist "%ISCC_EXE%" (
  echo Inno Setup 6 was not found. Set ISCC_EXE to the full path of ISCC.exe.
  exit /b 1
)

echo Building Stremio %APP_VERSION%...
if not defined SKIP_BUILD (
  cargo build --release --package stremio-native
  if errorlevel 1 exit /b %errorlevel%
) else (
  echo Reusing the completed Cargo release build.
)

echo Staging the app-local MSVC runtime...
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stage_windows_msvc_runtime.ps1 -BuildRoot "%CD%\target\release"
if errorlevel 1 exit /b %errorlevel%

echo Creating the installer...
"%ISCC_EXE%" "/DAppVersion=%APP_VERSION%" "/DBuildRoot=%CD%\target\release" "/DArtifactsDir=%CD%\artifacts" setup\StremioNative.iss
if errorlevel 1 exit /b %errorlevel%

set "INSTALLER=%CD%\artifacts\StremioSetup-v%APP_VERSION%-x64.exe"
set "UPDATER_PACKAGE_DIR=%CD%\artifacts\updater-package"
set "UPDATER_ARCHIVE=%CD%\artifacts\stremio-native-v%APP_VERSION%-x86_64-pc-windows-msvc.zip"

if not exist "%INSTALLER%" (
  echo Expected installer was not created: %INSTALLER%
  exit /b 1
)

if not exist "%UPDATER_PACKAGE_DIR%" mkdir "%UPDATER_PACKAGE_DIR%"
if exist "%UPDATER_PACKAGE_DIR%\stremio-installer.exe" del /q "%UPDATER_PACKAGE_DIR%\stremio-installer.exe"
copy /y "%INSTALLER%" "%UPDATER_PACKAGE_DIR%\stremio-installer.exe" >nul
if errorlevel 1 exit /b %errorlevel%

if exist "%UPDATER_ARCHIVE%" del /q "%UPDATER_ARCHIVE%"
tar -a -c -f "%UPDATER_ARCHIVE%" -C "%UPDATER_PACKAGE_DIR%" stremio-installer.exe
if errorlevel 1 exit /b %errorlevel%

del /q "%UPDATER_PACKAGE_DIR%\stremio-installer.exe"
rmdir "%UPDATER_PACKAGE_DIR%"

echo Installer and updater archive created in %CD%\artifacts.
