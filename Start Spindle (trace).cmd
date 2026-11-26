@echo off
rem Start Spindle and record every Ollama request and reply in the traces folder here,
rem one JSON line per request (traces\trace-YYYY-MM-DD.jsonl). Close Spindle to stop.
set "SPINDLE_TRACE=%~dp0traces"
call "%~dp0Start Spindle.cmd"
