@echo off
chcp 65001 >nul

:menu
echo ========================================
echo   ArtAIT Rust build script
echo ========================================
echo.
echo [1] Release build        cargo build --release
echo [2] Quick check          cargo check
echo [3] Fast debug build app cargo build -p artait-app --bin ArtForgeStudio --profile dev-fast
echo [4] Build and run app    build dev-fast, copy to target\debug-run then start
echo [5] Clippy check         cargo clippy
echo [6] Small release build  cargo build --release (opt-level="s")
echo [7] Exit
echo.

choice /C 1234567 /N /M "Choose [1-7]: "
set "choice=%ERRORLEVEL%"

if "%choice%"=="1" goto release
if "%choice%"=="2" goto check
if "%choice%"=="3" goto debug
if "%choice%"=="4" goto run
if "%choice%"=="5" goto clippy
if "%choice%"=="6" goto release_small
if "%choice%"=="7" exit /b 0

goto menu

:release
echo.
echo RUN: cargo build --release
cargo build --release
if errorlevel 1 (
    echo ERROR: build failed.
    call :build_failed_hint
    pause
    goto menu
)
echo OK: build succeeded.
for %%F in (target\release\ArtForgeStudio.exe) do echo    App: %%F (%%~zF bytes)
for %%F in (target\release\artait-migrate.exe) do echo    Migrate: %%F (%%~zF bytes)
pause
goto menu

:release_small
echo.
echo RUN: cargo build --release (opt-level="s")
cargo build --release --config 'profile.release.opt-level="s"'
if errorlevel 1 (
    echo ERROR: build failed.
    call :build_failed_hint
    pause
    goto menu
)
echo OK: small build succeeded.
for %%F in (target\release\ArtForgeStudio.exe) do echo    App: %%F (%%~zF bytes)
for %%F in (target\release\artait-migrate.exe) do echo    Migrate: %%F (%%~zF bytes)
pause
goto menu

:check
echo.
echo RUN: cargo check
cargo check
pause
goto menu

:debug
echo.
call :ensure_dev_output_unlocked
if errorlevel 1 goto menu
call :setup_fast_linker
echo RUN: cargo build -p artait-app --bin ArtForgeStudio --profile dev-fast
cargo build -p artait-app --bin ArtForgeStudio --profile dev-fast
pause
goto menu

:run
echo.
call :ensure_dev_output_unlocked
if errorlevel 1 goto menu
call :setup_fast_linker
echo RUN: cargo build -p artait-app --bin ArtForgeStudio --profile dev-fast
cargo build -p artait-app --bin ArtForgeStudio --profile dev-fast
if errorlevel 1 (
    echo ERROR: build failed.
    call :build_failed_hint
    pause
    goto menu
)
if not exist target\debug-run mkdir target\debug-run
set "run_exe=target\debug-run\ArtForgeStudio-dev-%RANDOM%.exe"
copy /Y target\dev-fast\ArtForgeStudio.exe "%run_exe%" >nul
if errorlevel 1 (
    echo ERROR: failed to copy debug exe.
    pause
    goto menu
)
echo RUN: %run_exe%
start "" "%run_exe%"
goto menu

:clippy
echo.
echo RUN: cargo clippy
cargo clippy
pause
goto menu

:build_failed_hint
echo.
echo If you see "os error 32" or "file is being used by another process":
echo   1. Close running ArtForgeStudio.exe or Explorer preview windows.
echo   2. Wait a few seconds, then build again.
echo   3. If release is still locked, run:
echo      cargo build --release --target-dir target\release-verify
echo.
echo Dev note:
echo   Do not launch target\dev-fast\ArtForgeStudio.exe directly.
echo   Option 4 starts a copied exe from target\debug-run to avoid locking Cargo output.
echo.
exit /b 0

:ensure_dev_output_unlocked
powershell -NoProfile -ExecutionPolicy Bypass -Command "$target=[IO.Path]::GetFullPath((Join-Path (Get-Location) 'target\dev-fast\ArtForgeStudio.exe')); $locked=@(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'ArtForgeStudio.exe' -and $_.ExecutablePath -and ([IO.Path]::GetFullPath($_.ExecutablePath) -ieq $target) }); if ($locked.Count -gt 0) { Write-Host 'WARN: build output is running and will lock Cargo:'; $locked | ForEach-Object { Write-Host ('  PID {0}: {1}' -f $_.ProcessId, $_.ExecutablePath) }; exit 32 }"
set "lock_status=%ERRORLEVEL%"
if "%lock_status%"=="0" exit /b 0
if "%lock_status%"=="32" (
    echo.
    choice /C YN /N /M "Close target\dev-fast\ArtForgeStudio.exe now and continue? [Y/N]: "
    if errorlevel 2 exit /b 1
    powershell -NoProfile -ExecutionPolicy Bypass -Command "$target=[IO.Path]::GetFullPath((Join-Path (Get-Location) 'target\dev-fast\ArtForgeStudio.exe')); $locked=@(Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'ArtForgeStudio.exe' -and $_.ExecutablePath -and ([IO.Path]::GetFullPath($_.ExecutablePath) -ieq $target) }); if ($locked.Count -gt 0) { Stop-Process -Id ($locked | ForEach-Object ProcessId) -Force; Start-Sleep -Milliseconds 500 }"
    if errorlevel 1 exit /b 1
    exit /b 0
)
echo ERROR: failed to check whether target\dev-fast\ArtForgeStudio.exe is locked.
exit /b 1

:setup_fast_linker
set "RUST_SYSROOT="
set "RUST_LLD="
for /f "delims=" %%P in ('rustc --print sysroot') do set "RUST_SYSROOT=%%P"
if defined RUST_SYSROOT set "RUST_LLD=%RUST_SYSROOT%\lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe"
if exist "%RUST_LLD%" (
    echo %RUSTFLAGS% | findstr /C:"%RUST_LLD%" >nul
    if errorlevel 1 (
        if defined RUSTFLAGS (
            set "RUSTFLAGS=%RUSTFLAGS% -C linker=%RUST_LLD%"
        ) else (
            set "RUSTFLAGS=-C linker=%RUST_LLD%"
        )
        echo INFO: dev-fast linker: %RUST_LLD%
    ) else (
        echo INFO: dev-fast linker already configured.
    )
)
exit /b 0
