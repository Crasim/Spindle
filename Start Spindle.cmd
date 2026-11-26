@echo off
setlocal
cd /d "%~dp0"
if not exist "target\release\spindle.exe" if not exist "target\staged\release\spindle.exe" if not exist "target\panels\release\spindle.exe" (
    set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    cargo build --release --locked
    if errorlevel 1 (
        pause
        exit /b 1
    )
)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\start-spindle.ps1"
if errorlevel 1 (
    pause
    exit /b 1
)
