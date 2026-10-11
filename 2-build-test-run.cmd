@echo off
cd /d "%~dp0"
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-and-run.ps1" -Test -Run
if errorlevel 1 echo Build failed. Read the error above.
pause
