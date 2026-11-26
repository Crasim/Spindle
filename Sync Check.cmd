@echo off
rem Ask the running Spindle to try each of your other devices and save what happened.
cd /d "%~dp0"
set "EXE=target\release\spindle.exe"
if not exist "%EXE%" set "EXE=target\staged\release\spindle.exe"
"%EXE%" sync check > sync-check.txt 2>&1
type sync-check.txt
pause
