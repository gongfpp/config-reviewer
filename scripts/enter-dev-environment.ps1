# Dot-source this file. All changes are process-local; no global PATH/rustup changes.
$ErrorActionPreference = 'Stop'
$kitRoot = Split-Path -Parent $PSScriptRoot
$statePath = Join-Path $kitRoot '.offline/environment.json'
if (-not (Test-Path $statePath)) { throw 'Run 1-install-environment.cmd first.' }
$devState = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
$node = Join-Path $kitRoot 'tools/node'
$rust = Join-Path $kitRoot 'tools/rust/bin'
$devcmd = Join-Path $devState.msvc_install_path 'Common7/Tools/VsDevCmd.bat'
foreach ($required in @((Join-Path $node 'node.exe'),(Join-Path $rust 'cargo.exe'),(Join-Path $rust 'rustc.exe'),$devcmd)) {
  if (-not (Test-Path -LiteralPath $required)) { throw "Missing offline development tool: $required" }
}
$bootstrap = Join-Path ([IO.Path]::GetTempPath()) ('reviewer-env-' + [Guid]::NewGuid().ToString('N') + '.cmd')
try {
  # Carry Unicode paths through the environment rather than an OEM-encoded batch file.
  [IO.File]::WriteAllLines($bootstrap,@('@echo off','call "%REVIEWER_VS_DEVCMD%" -no_logo -arch=x64 -host_arch=x64 -winsdk=10.0.19041.0 >nul','if errorlevel 1 exit /b %errorlevel%','set'),[Text.Encoding]::ASCII)
  $start = New-Object Diagnostics.ProcessStartInfo
  $start.FileName = Join-Path $env:WINDIR 'System32/cmd.exe'
  $start.Arguments = '/u /d /c call "' + $bootstrap + '"'
  $start.UseShellExecute = $false
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  $start.StandardOutputEncoding = [Text.Encoding]::Unicode
  $start.StandardErrorEncoding = [Text.Encoding]::Unicode
  $start.EnvironmentVariables['REVIEWER_VS_DEVCMD'] = $devcmd
  $process = New-Object Diagnostics.Process
  $process.StartInfo = $start
  [void]$process.Start()
  $output = $process.StandardOutput.ReadToEnd()
  $errorOutput = $process.StandardError.ReadToEnd()
  $process.WaitForExit()
  if ($process.ExitCode -ne 0) { throw "Cannot initialize the packaged C++ toolchain: $errorOutput" }
  $environment = $output -split '\r?\n'
} finally {
  if ($process) { $process.Dispose() }
  if (Test-Path $bootstrap) { [IO.File]::Delete($bootstrap) }
}
foreach ($line in $environment) {
  if ($line -match '^([^=]+)=(.*)$' -and $matches[1] -ne 'REVIEWER_VS_DEVCMD') { [Environment]::SetEnvironmentVariable($matches[1],$matches[2],'Process') }
}
$env:PATH = "$node;$rust;$env:PATH"
$env:CARGO_HOME = Join-Path $kitRoot '.offline/cargo-home'
$env:RUSTC = Join-Path $rust 'rustc.exe'
$env:RUSTDOC = Join-Path $rust 'rustdoc.exe'
$env:CARGO_NET_OFFLINE = 'true'
$env:npm_config_offline = 'true'
$env:npm_config_cache = Join-Path $kitRoot '.offline/npm-cache'
$env:CC = (Get-Command cl.exe -ErrorAction Stop).Source
$env:AR = (Get-Command lib.exe -ErrorAction Stop).Source
$env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = (Get-Command link.exe -ErrorAction Stop).Source
if (-not $env:CC.StartsWith($devState.msvc_install_path,[StringComparison]::OrdinalIgnoreCase)) { throw 'Unexpected global compiler selected.' }
if ($env:WindowsSDKVersion.TrimEnd('\') -ne '10.0.19041.0') { throw 'Packaged Windows 10 SDK was not selected.' }
New-Item -ItemType Directory -Force $env:CARGO_HOME | Out-Null
