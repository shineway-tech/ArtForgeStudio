@echo off
chcp 65001 >nul

:menu
echo ========================================
echo   Elunvi Canvas build script
echo ========================================
echo.
echo [1] Release build       ElunviCanvas
echo [2] Quick check         ElunviCanvas
echo [3] Run                 ElunviCanvas
echo [4] Small release build ElunviCanvas
echo [5] Clippy              ElunviCanvas
echo [6] Exit
echo.

choice /C 123456 /N /M "Choose [1-6]: "
set "choice=%ERRORLEVEL%"

if "%choice%"=="1" goto release
if "%choice%"=="2" goto check
if "%choice%"=="3" goto run
if "%choice%"=="4" goto release_small
if "%choice%"=="5" goto clippy
if "%choice%"=="6" exit /b 0

goto menu

:release
echo.
echo RUN: cargo build --release -p artforge-studio-native --bin ElunviCanvas
cargo build --release -p artforge-studio-native --bin ElunviCanvas
if errorlevel 1 (
    echo ERROR: build failed.
    call :build_failed_hint
    pause
    goto menu
)
echo OK: build succeeded.
for %%F in (target\release\ElunviCanvas.exe) do echo    %%F (%%~zF bytes)
pause
goto menu

:release_small
echo.
echo RUN: cargo build --release -p artforge-studio-native --bin ElunviCanvas (opt-level="s")
cargo build --release -p artforge-studio-native --bin ElunviCanvas --config "profile.release.opt-level='s'"
if errorlevel 1 (
    echo ERROR: build failed.
    call :build_failed_hint
    pause
    goto menu
)
echo OK: small build succeeded.
for %%F in (target\release\ElunviCanvas.exe) do echo    %%F (%%~zF bytes)
pause
goto menu

:check
echo.
echo RUN: cargo check -p artforge-studio-native
cargo check -p artforge-studio-native
pause
goto menu

:run
echo.
echo RUN: cargo run -p artforge-studio-native --bin ElunviCanvas
cargo run -p artforge-studio-native --bin ElunviCanvas
pause
goto menu

:clippy
echo.
echo RUN: cargo clippy -p artforge-studio-native
cargo clippy -p artforge-studio-native
pause
goto menu

:build_failed_hint
echo.
echo If you see "os error 32" or "file is being used by another process":
echo   1. Close running ElunviCanvas.exe or Explorer preview windows.
echo   2. Wait a few seconds, then build again.
echo   3. If release is still locked, run:
echo      cargo build --release -p artforge-studio-native --target-dir target\release-verify
echo.
exit /b 0
