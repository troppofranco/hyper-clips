@echo off
rem Hyper Clips - automatic check: build, tests, clippy, run the app, measure RAM.
rem Put this file in the project root (next to Cargo.toml) and double-click it.
rem Everything is written to check-report.txt: send me that file.

setlocal
cd /d "%~dp0"
set "LOG=%~dp0check-report.txt"
set "PS=powershell -NoProfile -ExecutionPolicy Bypass"

echo Hyper Clips check > "%LOG%"
echo Started: %DATE% %TIME% >> "%LOG%"
echo. >> "%LOG%"

where cargo >nul 2>&1
if errorlevel 1 (
    echo [FAIL] cargo not found. Install Rust from https://rustup.rs and reopen this window.
    echo cargo not found >> "%LOG%"
    goto :end
)
cargo --version >> "%LOG%" 2>&1

rem Close any copy left running from a previous try, so the build can overwrite the exe.
taskkill /f /im hyperclips.exe >nul 2>&1
taskkill /f /im hyperclips-ui.exe >nul 2>&1

echo ===== 1/4 BUILD (release) ===== >> "%LOG%"
echo [1/4] Building (the first time this takes several minutes)...
cargo build --release --workspace >> "%LOG%" 2>&1
if errorlevel 1 (
    echo [FAIL] build failed. Nothing else can run. See check-report.txt
    goto :end
)
echo [ OK ] build

echo. >> "%LOG%"
echo ===== 2/4 TESTS ===== >> "%LOG%"
echo [2/4] Running tests...
cargo test --workspace >> "%LOG%" 2>&1
if errorlevel 1 (echo [FAIL] tests) else (echo [ OK ] tests)

echo. >> "%LOG%"
echo ===== 3/4 CLIPPY ===== >> "%LOG%"
echo [3/4] Running clippy...
cargo clippy --workspace --all-targets -- -D warnings >> "%LOG%" 2>&1
if errorlevel 1 (echo [FAIL] clippy) else (echo [ OK ] clippy)

echo. >> "%LOG%"
echo ===== 4/4 RUN AND MEASURE RAM ===== >> "%LOG%"
echo [4/4] Starting the service, waiting 45 seconds, then measuring...
start "" "target\release\hyperclips.exe"
timeout /t 45 /nobreak >nul

echo --- service only (UI closed) --- >> "%LOG%"
%PS% -File "scripts\measure-memory.ps1" >> "%LOG%" 2>&1

echo Opening the UI, waiting 20 seconds, then measuring again...
start "" "target\release\hyperclips-ui.exe"
timeout /t 20 /nobreak >nul

echo --- service + UI open --- >> "%LOG%"
%PS% -File "scripts\measure-memory.ps1" >> "%LOG%" 2>&1

echo. >> "%LOG%"
echo ===== SERVICE LOG ===== >> "%LOG%"
%PS% -Command "$d = Join-Path $env:APPDATA 'HyperClips\logs'; if (Test-Path $d) { Get-ChildItem $d -Filter 'service*' | Sort-Object LastWriteTime | Select-Object -Last 1 | Get-Content } else { 'no log folder at ' + $d }" >> "%LOG%" 2>&1

taskkill /f /im hyperclips-ui.exe >nul 2>&1
taskkill /f /im hyperclips.exe >nul 2>&1
echo [ OK ] measurements done

:end
echo. >> "%LOG%"
echo Finished: %DATE% %TIME% >> "%LOG%"
echo.
echo Done. Send me the file: %LOG%
pause
endlocal
