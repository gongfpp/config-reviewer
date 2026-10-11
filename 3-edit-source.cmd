@echo off
cd /d "%~dp0"
if not exist "%~dp0tools\editor\Code.exe" (
  echo Offline editor missing. Extract the complete development kit.
  pause
  exit /b 1
)
start "" "%~dp0tools\editor\Code.exe" "%~dp0"
