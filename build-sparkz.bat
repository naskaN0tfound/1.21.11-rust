@echo off
setlocal EnableDelayedExpansion
title Sparkz builder
echo ============================================================
echo  Builds Sparkz.exe (Windows 10/11, 64-bit). First run takes
echo  15-40 minutes and downloads several GB. Leave it open.
echo ============================================================
echo.

cd /d "%~dp0"
set "SRC="
if exist "launcher\Cargo.toml" set "SRC=%~dp0"
if not defined SRC if exist "horizon-main\launcher\Cargo.toml" set "SRC=%~dp0horizon-main\"
if not defined SRC (
  if exist "horizon-1.21.11.zip" (
    echo Unpacking horizon-1.21.11.zip ...
    tar -xf horizon-1.21.11.zip
    if exist "horizon-main\launcher\Cargo.toml" set "SRC=%~dp0horizon-main\"
  )
)
if not defined SRC (
  echo ERROR: put this file next to horizon-1.21.11.zip, or inside the horizon-main folder.
  pause & exit /b 1
)
echo Source: %SRC%

rem --- Microsoft sign-in: your own Azure app's Application (client) ID
set "IDFILE=%~dp0sparkz-client-id.txt"
set "MSAID="
if exist "%IDFILE%" set /p MSAID=<"%IDFILE%"
if not defined MSAID (
  echo.
  echo To log in with your Microsoft account Sparkz needs YOUR Azure app's client ID ^(a GUID like
  echo 12345678-aaaa-bbbb-cccc-1234567890ab^). See SPARKZ-LOGIN.txt for the 5-minute setup.
  echo Press Enter with nothing typed to build without sign-in for now ^(offline names only^).
  set /p MSAID=Client ID: 
  if defined MSAID echo !MSAID!>"%IDFILE%"
)
if defined MSAID ( set "HORIZON_MSA_CLIENT_ID=!MSAID!" & echo Sign-in client ID: !MSAID! ) else echo Building without Microsoft sign-in.

rem --- Visual Studio Build Tools (C++ linker and Windows SDK)
set "VSW=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "HAVE_MSVC="
if exist "%VSW%" for /f "usebackq delims=" %%i in (`"%VSW%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "HAVE_MSVC=%%i"
if not defined HAVE_MSVC (
  echo Installing Visual Studio Build Tools ^(C++ and Windows SDK^) ...
  winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-package-agreements --accept-source-agreements --override "--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  if errorlevel 1 (
    echo Build Tools install failed or was cancelled. If winget is missing, install "App Installer" from the Microsoft Store.
    pause & exit /b 1
  )
) else echo Build Tools found.

rem --- Rust
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
  echo Downloading Rust installer ...
  powershell -NoProfile -Command "Invoke-WebRequest https://win.rustup.rs/x86_64 -OutFile '%TEMP%\rustup-init.exe'"
  if errorlevel 1 ( echo Download failed. Check your internet. & pause & exit /b 1 )
  "%TEMP%\rustup-init.exe" -y --default-toolchain stable --profile minimal
  if errorlevel 1 ( echo Rust install failed. & pause & exit /b 1 )
)
call rustup update stable
call rustup default stable

rem --- Build (C runtime linked in: runs on a PC without the Visual C++ redistributable)
set "RUSTFLAGS=-C target-feature=+crt-static"
cd /d "%SRC%launcher"
echo.
echo Building Sparkz. Cargo downloads every Rust library it needs ...
cargo build --release
if errorlevel 1 (
  echo.
  echo BUILD FAILED. Scroll up for the first "error" and send me that text.
  pause & exit /b 1
)

copy /y "target\release\horizon-launcher.exe" "%~dp0Sparkz.exe" >nul
if errorlevel 1 ( echo Could not copy the exe. & pause & exit /b 1 )
echo.
echo ============================================================
echo  Done: %~dp0Sparkz.exe
echo ============================================================
pause
