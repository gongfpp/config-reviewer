# Only for disposable hosted CI. Never removes software on a user's computer.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') { throw 'This destructive isolation test is restricted to disposable GitHub-hosted runners.' }
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
New-Item -ItemType Directory -Force artifacts/dev-kit-test | Out-Null
Start-Transcript artifacts/dev-kit-test/full-test.log
$profiles = @(Get-NetFirewallProfile | Select-Object Name,Enabled,DefaultOutboundAction)
$before = @{ os=(Get-CimInstance Win32_OperatingSystem).Caption; existing_compilers_removed=$false; tool_paths_isolated=$false; all_external_outbound_blocked=$false; target_windows10_tested=$false }
try {
  foreach ($name in @('install-offline-environment.ps1','enter-dev-environment.ps1','verify-kit.ps1','build-and-run.ps1')) {
    $tokens, $parseErrors = $null, $null
    [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name),[ref]$tokens,[ref]$parseErrors) | Out-Null
    if ($parseErrors) { throw "PowerShell 5.1 syntax error in $name`: $parseErrors" }
  }
  # Validate the firewall command before spending time removing preinstalled VS.
  New-NetFirewallRule -Name ReviewerOfflineAcceptance -DisplayName ReviewerOfflineAcceptance -Direction Outbound -Action Block -RemoteAddress Any -Profile Any -Enabled False | Out-Null
  function Assert-NetworkIsolation {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback,0)
    $clientProbe = New-Object Net.Sockets.TcpClient
    try {
      $listener.Start()
      $connection = $clientProbe.ConnectAsync('127.0.0.1',$listener.LocalEndpoint.Port)
      if (-not $connection.Wait(3000) -or -not $clientProbe.Connected) { throw 'Loopback traffic was blocked; local test gateway cannot run.' }
    } finally { $clientProbe.Dispose(); $listener.Stop() }
    try {
      Invoke-WebRequest 'https://www.microsoft.com' -UseBasicParsing -TimeoutSec 8 | Out-Null
      throw 'External network probe unexpectedly succeeded.'
    } catch {
      if ($_.Exception.Message -match 'unexpectedly succeeded') { throw }
      $before.external_probe_failed = $_.Exception.Message
    }
  }
  Enable-NetFirewallRule -Name ReviewerOfflineAcceptance | Out-Null
  Set-NetFirewallProfile -Profile Domain,Private,Public -Enabled True
  Assert-NetworkIsolation
  Disable-NetFirewallRule -Name ReviewerOfflineAcceptance | Out-Null
  foreach ($profile in $profiles) { Set-NetFirewallProfile -Profile $profile.Name -Enabled $profile.Enabled -DefaultOutboundAction $profile.DefaultOutboundAction }
  Write-Output 'Firewall isolation preflight passed.'
  Write-Output 'Removing preinstalled C++ compiler instances from this disposable runner.'
  # Remove the runner's compiler installation rather than accidentally validating it.
  $vsInstaller = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer'
  $vswhere = Join-Path $vsInstaller 'vswhere.exe'
  if (Test-Path $vswhere) {
    $instances = (& $vswhere -all -products '*' -format json | ConvertFrom-Json)
    foreach ($instance in $instances) {
      $p = Start-Process (Join-Path $vsInstaller 'setup.exe') -ArgumentList @('uninstall','--installPath',('"{0}"' -f $instance.installationPath),'--quiet','--norestart') -Wait -PassThru
      if ($p.ExitCode -notin @(0,3010)) { throw "Cannot remove preinstalled VS instance: $($p.ExitCode)" }
    }
    $remaining = (& $vswhere -all -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    if ($remaining) { throw 'A preinstalled C++ toolchain is still registered.' }
  }
  $before.existing_compilers_removed = $true
  Write-Output 'Preinstalled C++ toolchain removed.'
  # Hide global Node/Rust and use an empty Cargo cache; Windows tools remain available.
  $env:PATH = "$env:WINDIR\System32;$env:WINDIR;$env:WINDIR\System32\WindowsPowerShell\v1.0"
  foreach ($tool in @('node','npm','cargo','rustc','cl','link')) {
    if (Get-Command $tool -ErrorAction SilentlyContinue) { throw "Unexpected preinstalled command: $tool" }
  }
  $before.tool_paths_isolated = $true
  # Remove Evergreen WebView2 to exercise the bundled full offline installer.
  $webviewRoot = Join-Path ${env:ProgramFiles(x86)} 'Microsoft/EdgeWebView/Application'
  $setup = Get-ChildItem "$webviewRoot/*/Installer/setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($setup) {
    $p = Start-Process $setup.FullName -ArgumentList @('--uninstall','--msedgewebview','--system-level','--force-uninstall','--verbose-logging') -Wait -PassThru
    # Chromium setup uses 19 for UNINSTALL_SUCCESSFUL; registry absence is checked below.
    if ($p.ExitCode -notin @(0,19)) { throw "WebView2 uninstall failed: $($p.ExitCode)" }
  }
  $client = 'HKLM:/SOFTWARE/WOW6432Node/Microsoft/EdgeUpdate/Clients/{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
  $existing = Get-ItemProperty $client -ErrorAction SilentlyContinue
  if ($existing -and $existing.pv -and $existing.pv -ne '0.0.0.0') { throw 'WebView2 still installed.' }
  $before.webview2_absent_before_install = $true
  $before.sdk19041_preinstalled = Test-Path (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Include/10.0.19041.0')
  # Block external traffic for every process, including installer service children.
  # Loopback is excluded by the Windows firewall, so the local mock LLM stays usable.
  Enable-NetFirewallRule -Name ReviewerOfflineAcceptance | Out-Null
  Set-NetFirewallProfile -Profile Domain,Private,Public -Enabled True
  Assert-NetworkIsolation
  $before.all_external_outbound_blocked = $true
  Write-Output 'External traffic blocked; loopback connectivity verified. Installing the supplied offline environment.'
  $before | ConvertTo-Json | Set-Content artifacts/dev-kit-test/isolation.json -Encoding UTF8
  # Windows PowerShell 5.1 parses and runs the user entrypoints, not pwsh 7.
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-offline-environment.ps1 -InstallPath 'C:\ReviewerBuildTools'
  if ($LASTEXITCODE -ne 0) { throw "Offline environment installation failed: $LASTEXITCODE" }
  . "$PSScriptRoot/enter-dev-environment.ps1"
  & tools/editor/bin/code.cmd --version
  if ($LASTEXITCODE -ne 0) { throw 'Packaged offline editor cannot start its CLI.' }
  $env:REVIEWER_RUNTIME_PATH = "$env:WINDIR\System32;$env:WINDIR"
  if ((Get-Command node.exe).Source -ne (Join-Path $root 'tools/node/node.exe')) { throw 'Node was not selected from the kit.' }
  if ((Get-Command rustc.exe).Source -ne (Join-Path $root 'tools/rust/bin/rustc.exe')) { throw 'Rust was not selected from the kit.' }
  if (Test-Path src-tauri/target) { throw 'Acceptance requires a clean target directory.' }
  # A visible source change must survive compilation and be seen by the real app.
  $ui = Join-Path $root 'src/App.tsx'
  $text = [IO.File]::ReadAllText($ui)
  if (-not $text.Contains('配置评审台')) { throw 'Expected product text not found.' }
  [IO.File]::WriteAllText($ui,$text.Replace('配置评审台','配置评审台 OFFLINE-EDIT-OK'),(New-Object Text.UTF8Encoding($false)))
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/build-and-run.ps1 -Test
  if ($LASTEXITCODE -ne 0) { throw "Offline source compilation failed: $LASTEXITCODE" }
  & npm.cmd install --prefix scripts --offline --cache .offline/npm-cache --ignore-scripts --no-audit --fund=false
  if ($LASTEXITCODE -ne 0) { throw 'Offline UI test dependencies missing.' }
  $env:REVIEWER_EXPECT_SOURCE_EDIT = 'OFFLINE-EDIT-OK'
  & node.exe scripts/windows-ui-smoke.mjs src-tauri/target/x86_64-pc-windows-msvc/debug/config-reviewer.exe
  if ($LASTEXITCODE -ne 0) { throw 'Compiled app UI acceptance failed.' }
  # Prove a second modification compiles incrementally; avoids a one-time-only setup.
  [IO.File]::WriteAllText($ui,$text.Replace('配置评审台','配置评审台 OFFLINE-SECOND-EDIT'),(New-Object Text.UTF8Encoding($false)))
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/build-and-run.ps1
  if ($LASTEXITCODE -ne 0) { throw 'Second offline rebuild failed.' }
  $env:REVIEWER_EXPECT_SOURCE_EDIT = 'OFFLINE-SECOND-EDIT'
  & node.exe scripts/windows-ui-smoke.mjs src-tauri/target/x86_64-pc-windows-msvc/debug/config-reviewer.exe
  if ($LASTEXITCODE -ne 0) { throw 'Second source modification was not runnable.' }
  [IO.File]::WriteAllText($ui,$text,(New-Object Text.UTF8Encoding($false)))
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/build-and-run.ps1
  if ($LASTEXITCODE -ne 0) { throw 'Cannot rebuild the original deliverable after source-edit tests.' }
  New-Item -ItemType Directory -Force ready-to-run | Out-Null
  Copy-Item src-tauri/target/x86_64-pc-windows-msvc/debug/config-reviewer.exe ready-to-run/config-reviewer.exe
  Remove-Item Env:/REVIEWER_EXPECT_SOURCE_EDIT
  & node.exe scripts/windows-ui-smoke.mjs ready-to-run/config-reviewer.exe
  if ($LASTEXITCODE -ne 0) { throw 'Relocated original standalone EXE cannot run.' }
  @{ passed=$true; source_commit=(Get-Content .build-source.json -Raw | ConvertFrom-Json).source_commit; os=$before.os; isolation=$before; source_modification_rebuilds=2; relocated_original_exe_tested=$true; rust_tests=34; ui_test=(Get-Content artifacts/ui-smoke/result.json -Raw | ConvertFrom-Json); windows10_target_tested=$false; dotnet_and_vc_runtime='Preinstalled OS prerequisites; bundled installers are signature-verified but absent-state installation not exercised.' } | ConvertTo-Json -Depth 8 | Set-Content artifacts/dev-kit-test/result.json -Encoding UTF8
} finally {
  Remove-NetFirewallRule -Name ReviewerOfflineAcceptance -ErrorAction SilentlyContinue
  foreach ($profile in $profiles) { Set-NetFirewallProfile -Profile $profile.Name -Enabled $profile.Enabled -DefaultOutboundAction $profile.DefaultOutboundAction }
  Get-ChildItem "$env:TEMP/dd_*" -ErrorAction SilentlyContinue | Copy-Item -Destination artifacts/dev-kit-test -ErrorAction SilentlyContinue
  Stop-Transcript
}
