@echo off
cd /d "%~dp0"
if not exist "%~dp0ready-to-run\config-reviewer.exe" (
  echo Prebuilt app missing. Run 2-build-test-run.cmd to compile the source.
  pause
  exit /b 1
)
start "" "%~dp0ready-to-run\config-reviewer.exe"
