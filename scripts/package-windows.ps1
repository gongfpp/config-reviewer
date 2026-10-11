# 在 Windows 构建成功后生成可导入内网的完整压缩包和哈希清单。
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $root 'package.json') -Raw | ConvertFrom-Json).version
$stage = Join-Path $root 'artifacts/windows-offline'
New-Item -ItemType Directory -Path $stage -Force | Out-Null
$installers = @(Get-ChildItem (Join-Path $root 'src-tauri/target/release/bundle/nsis') -Filter '*-setup.exe')
if ($installers.Count -ne 1) { throw 'Expected exactly one NSIS installer.' }
if ($installers[0].Length -lt 50MB) { throw 'Installer is unexpectedly small: offline WebView2 may be missing.' }
Copy-Item $installers[0].FullName $stage
$executable = Join-Path $root 'src-tauri/target/x86_64-pc-windows-msvc/release/config-reviewer.exe'
if (-not (Test-Path $executable)) { throw 'Release executable not found.' }
Copy-Item $executable $stage
Copy-Item (Join-Path $root 'docs/third-party') (Join-Path $stage 'third-party') -Recurse -Force
Copy-Item (Join-Path $root 'docs/intranet-deployment.md') (Join-Path $stage '安装与兼容性说明.md')
Copy-Item (Join-Path $root 'scripts/check-intranet.ps1') $stage
Copy-Item (Join-Path $root 'docs/compatibility-audit.md') $stage
$metadata = if (Test-Path (Join-Path $root '.build-source.json')) { Get-Content (Join-Path $root '.build-source.json') -Raw | ConvertFrom-Json } else { @{ source_commit = 'unknown' } }
$buildInfo = @{ version = $version; source_commit = $metadata.source_commit; platform = 'Windows x64'; webview_install_mode = 'offlineInstaller'; msvc_crt = 'static'; code_signed = $false }
$buildInfo | ConvertTo-Json | Set-Content (Join-Path $stage 'build-info.json') -Encoding utf8
$hashes = Get-ChildItem $stage -File -Recurse | Where-Object Name -ne 'SHA256SUMS.txt' | Sort-Object FullName | ForEach-Object { "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.FullName.Substring($stage.Length + 1).Replace('\', '/'))" }
$hashes | Set-Content (Join-Path $stage 'SHA256SUMS.txt') -Encoding utf8
$archive = Join-Path $root "artifacts/ConfigReviewer-$version-windows-x64-offline.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archive -Force
Write-Output $archive
