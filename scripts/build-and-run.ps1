# Windows PowerShell 5.1 / PowerShell 7；只构建应用，不生成安装包。
[CmdletBinding()]
param([switch]$Release, [switch]$Run, [switch]$Test, [switch]$Online)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
foreach ($tool in @('node','npm','cargo','rustc')) {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "Required tool not found: $tool. See docs/onsite-development.md." }
}
if (-not [Environment]::Is64BitOperatingSystem) { throw 'Windows x64 is required.' }
# 从 VS Build Tools / Visual Studio 导入 C++ 编译环境；无需修改全局环境变量。
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
  if (-not (Test-Path $vswhere)) { throw 'Visual Studio C++ Build Tools with Windows SDK are required.' }
  $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if (-not $vs) { throw 'No MSVC x64 toolchain found.' }
  $devcmd = Join-Path $vs 'Common7/Tools/VsDevCmd.bat'
  $lines = & cmd.exe /s /c "`"`"$devcmd`" -no_logo -arch=x64 -host_arch=x64 >nul && set`""
  if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize MSVC environment.' }
  foreach ($line in $lines) {
    if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
  }
}
$offline = -not $Online
$cache = Join-Path $root '.offline/npm-cache'
if ($offline -and -not (Test-Path (Join-Path $root '.offline/cargo-vendor'))) {
  throw 'Offline dependency pack is missing. Extract it into this project before building.'
}
function Invoke-Checked([string]$File, [string[]]$Arguments) {
  & $File @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$File failed with exit code $LASTEXITCODE" }
}
$npmArgs = @('ci','--no-audit','--fund=false')
if ($offline) { $npmArgs += @('--offline','--cache',$cache) }
Invoke-Checked 'npm' $npmArgs
if ($Test) {
  Invoke-Checked 'npm' @('run','typecheck')
  $cargoArgs = @('test','--locked','--manifest-path','src-tauri/Cargo.toml')
  if ($offline) { $cargoArgs += '--offline' }
  Invoke-Checked 'cargo' $cargoArgs
}
$buildArgs = @('run','tauri','--','build','--no-bundle','--target','x86_64-pc-windows-msvc')
if (-not $Release) { $buildArgs += '--debug' }
$buildArgs += @('--','--locked')
if ($offline) { $buildArgs += '--offline' }
Invoke-Checked 'npm' $buildArgs
$profile = if ($Release) {'release'} else {'debug'}
$exe = Join-Path $root "src-tauri/target/x86_64-pc-windows-msvc/$profile/config-reviewer.exe"
if (-not (Test-Path $exe)) { throw 'Build finished without an executable.' }
Write-Output "Built application: $exe"
if ($Run) { Start-Process -FilePath $exe }
