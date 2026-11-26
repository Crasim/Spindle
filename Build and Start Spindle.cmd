@echo off
setlocal
rem Build the latest Spindle source, then open it in this folder's workspace.
rem   "Build and Start Spindle.cmd"        build and start
rem   "Build and Start Spindle.cmd" test   run the tests first and stop if any fail
cd /d "%~dp0"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>nul
if errorlevel 1 (
    echo Rust is not installed or cargo is not on PATH. Install it from https://rustup.rs
    pause
    exit /b 1
)

rem Windows locks a running exe, so build into whichever output folder is not in use.
set "TARGET=target\staged"
powershell.exe -NoProfile -Command "if (Get-Process spindle -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '*\target\staged\*' }) { exit 1 } else { exit 0 }"
if errorlevel 1 set "TARGET=target"

if /i "%~1"=="test" (
    echo Running tests...
    cargo test --release --locked --offline --target-dir "%TARGET%"
    if errorlevel 1 (
        echo.
        echo Tests failed. Spindle was not rebuilt.
        pause
        exit /b 1
    )
)

echo Building Spindle into %TARGET%...
cargo build --release --locked --offline --target-dir "%TARGET%"
if errorlevel 1 (
    echo Offline build failed. Retrying with network access for missing packages...
    cargo build --release --locked --target-dir "%TARGET%"
    if errorlevel 1 (
        echo.
        echo Build failed. The errors above show what to fix.
        pause
        exit /b 1
    )
)

rem One Spindle owns a workspace at a time. Offer to close the running copy.
powershell.exe -NoProfile -Command "if (Get-Process spindle -ErrorAction SilentlyContinue) { exit 1 } else { exit 0 }"
if errorlevel 1 (
    choice /c YN /m "Spindle is running. Close it and open the new build"
    if errorlevel 2 (
        echo The new build is ready. Close Spindle and run Start Spindle.cmd to use it.
        pause
        exit /b 0
    )
    powershell.exe -NoProfile -Command "$p = Get-Process spindle -ErrorAction SilentlyContinue; $p | ForEach-Object { [void]$_.CloseMainWindow() }; $p | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue; if (Get-Process spindle -ErrorAction SilentlyContinue) { exit 1 } else { exit 0 }"
    if errorlevel 1 (
        echo Spindle did not close. Close it yourself, then run Start Spindle.cmd.
        pause
        exit /b 1
    )
)

powershell.exe -NoProfile -Command "$build = Get-Item -LiteralPath '.\target\release\spindle.exe','.\target\staged\release\spindle.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1; Start-Process -FilePath $build.FullName -WorkingDirectory (Get-Location).Path"
