# Windows PowerShell 5.1. The kit includes every installer; no downloads occur here.
[CmdletBinding()]
param([string]$InstallPath = (Join-Path $env:ProgramFiles 'ConfigReviewerBuildTools'))
$ErrorActionPreference = 'Stop'
if (-not [Environment]::Is64BitProcess -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw 'Windows x64 and 64-bit PowerShell are required.' }
$os = Get-CimInstance Win32_OperatingSystem
if ([int]$os.BuildNumber -lt 18363) { throw 'This kit requires Windows 10 1909 (build 18363) or later, x64.' }
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
  $process = Start-Process "$env:WINDIR/System32/WindowsPowerShell/v1.0/powershell.exe" -Verb RunAs -Wait -PassThru -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',('"{0}"' -f $PSCommandPath),'-InstallPath',('"{0}"' -f $InstallPath))
  exit $process.ExitCode
}
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$logs = Join-Path $root '.offline/install-logs'
New-Item -ItemType Directory -Force $logs | Out-Null
Start-Transcript -Path (Join-Path $logs ('install-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log'))
try {
  & "$PSScriptRoot/verify-kit.ps1" -ToolsOnly
  $tools = Get-Content tools/TOOLS-INFO.json -Raw | ConvertFrom-Json
  if (-not $tools.vs_layout_verified -or -not $tools.toolchains_included) { throw 'Incomplete tool pack.' }
  if ($root.Length -gt 65) { throw 'Move the extracted kit to a short path such as C:\ReviewerKit before installation.' }
  $drive = Get-PSDrive -Name ([IO.Path]::GetPathRoot($InstallPath).Substring(0,1))
  if ($drive.Free -lt 15GB) { throw 'At least 15 GB free space is required on the compiler installation drive (30 GB recommended).' }
  # Only the official certificates carried in the verified Microsoft layout are imported.
  $certificates = @(Get-ChildItem tools/vs-layout/Certificates/*.cer)
  if ($certificates.Count -eq 0) { throw 'Offline Visual Studio signing certificates missing.' }
  foreach ($certificate in $certificates) {
    & certutil.exe -addstore -f Root $certificate.FullName
    if ($LASTEXITCODE -ne 0) { throw "Cannot install Microsoft signing certificate: $($certificate.Name)" }
  }
  function Install-Checked([string]$File,[string[]]$Arguments) {
    $process = Start-Process -FilePath (Join-Path $root $File) -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -notin @(0,3010,1641)) { throw "$File failed: $($process.ExitCode). See .offline/install-logs and %TEMP%/dd_*.log." }
    if ($process.ExitCode -in @(3010,1641)) {
      Write-Output 'REBOOT REQUIRED. Restart Windows and run 1-install-environment.cmd again before building.'
      exit 3010
    }
  }
  $net = Get-ItemProperty 'HKLM:/SOFTWARE/Microsoft/NET Framework Setup/NDP/v4/Full' -ErrorAction SilentlyContinue
  if (-not $net -or $net.Release -lt 528040) { Install-Checked 'tools/installers/ndp48-x86-x64-allos-enu.exe' @('/q','/norestart',('/log "{0}"' -f (Join-Path $logs 'dotnet.html'))) }
  # Newer installed VC runtime is accepted; do not downgrade existing machines.
  $vc = Get-ItemProperty 'HKLM:/SOFTWARE/Microsoft/VisualStudio/14.0/VC/Runtimes/x64' -ErrorAction SilentlyContinue
  $bundledVcVersion = [version](Get-Item tools/installers/vc_redist.x64.exe).VersionInfo.FileVersion
  if (-not $vc -or $vc.Installed -ne 1 -or [version]$vc.Version.TrimStart('v') -lt $bundledVcVersion) { Install-Checked 'tools/installers/vc_redist.x64.exe' @('/install','/quiet','/norestart',('/log "{0}"' -f (Join-Path $logs 'vc-runtime.log'))) }
  $webviewId = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
  $webview = Get-ItemProperty "HKLM:/SOFTWARE/WOW6432Node/Microsoft/EdgeUpdate/Clients/$webviewId" -ErrorAction SilentlyContinue
  if (-not $webview -or -not $webview.pv -or $webview.pv -eq '0.0.0.0') { Install-Checked 'tools/installers/MicrosoftEdgeWebView2RuntimeInstallerX64.exe' @('/silent','/install') }
  $vsArgs = @('--noWeb','--quiet','--wait','--norestart','--nocache','--installPath',('"{0}"' -f $InstallPath),'--addProductLang','en-US')
  foreach ($component in $tools.vs_components) { $vsArgs += @('--add',$component) }
  Install-Checked 'tools/vs-layout/vs_buildtools.exe' $vsArgs
  if (-not (Test-Path (Join-Path $InstallPath 'Common7/Tools/VsDevCmd.bat'))) { throw 'C++ installation did not produce VsDevCmd.bat.' }
  @{ msvc_install_path=$InstallPath; installed_at=(Get-Date).ToString('o'); os=$os.Caption; os_build=$os.BuildNumber; node=$tools.node; rust=$tools.rust; sdk='10.0.19041.0'; offline=$true } | ConvertTo-Json | Set-Content .offline/environment.json -Encoding UTF8
  . "$PSScriptRoot/enter-dev-environment.ps1"
  & node.exe --version
  if ($LASTEXITCODE -ne 0) { throw 'Packaged Node failed.' }
  & rustc.exe --version
  if ($LASTEXITCODE -ne 0) { throw 'Packaged Rust failed.' }
  & cargo.exe --version
  if ($LASTEXITCODE -ne 0) { throw 'Packaged Cargo failed.' }
  Write-Output 'Offline development environment is ready. Next: 2-build-test-run.cmd.'
} finally { Stop-Transcript }
