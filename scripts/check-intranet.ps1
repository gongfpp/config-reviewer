# Windows PowerShell 5.1；只检查本机环境和用户指定的内网地址。
[CmdletBinding()]
param([string]$BaseUrl = '', [string]$Model = '', [string]$ApiKeyHeader = '', [switch]$TestModel)
$ErrorActionPreference = 'Stop'
Write-Output "OS: $([Environment]::OSVersion.VersionString)"
Write-Output "64-bit OS: $([Environment]::Is64BitOperatingSystem)"
if (-not [Environment]::Is64BitOperatingSystem) { throw 'This package requires Windows x64.' }
$runtime = @()
foreach ($root in @('HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients','HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients','HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients')) {
  if (Test-Path $root) {
    $runtime += Get-ChildItem $root | ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.name -like '*WebView2*' -and $_.pv -and $_.pv -ne '0.0.0.0' }
  }
}
if ($runtime.Count) { $runtime | ForEach-Object { Write-Output "WebView2: $($_.pv)" } }
else { Write-Output 'WebView2: not detected. Use the offline setup EXE to install it.' }
Write-Output "User data directory: $env:APPDATA"
if (-not $BaseUrl) { return }
$uri = [Uri]$BaseUrl
if ($uri.Scheme -notin @('http','https')) { throw 'BaseUrl must be HTTP or HTTPS.' }
$port = if ($uri.IsDefaultPort) { if ($uri.Scheme -eq 'https') {443} else {80} } else {$uri.Port}
$reachable = Test-NetConnection -ComputerName $uri.DnsSafeHost -Port $port -InformationLevel Quiet -WarningAction SilentlyContinue
Write-Output "Gateway TCP reachable: $reachable"
if (-not $reachable) { throw 'Cannot reach the configured gateway.' }
if (-not $TestModel) { return }
if (-not $Model) { throw 'Model is required with -TestModel.' }
$secret = Read-Host 'API Key (never written to disk)' -AsSecureString
$pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secret)
$previousProxy = [System.Net.WebRequest]::DefaultWebProxy
try {
  [System.Net.WebRequest]::DefaultWebProxy = $null
  $key = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
  $headers = @{}
  if ($ApiKeyHeader) { $headers[$ApiKeyHeader] = $key } else { $headers['Authorization'] = "Bearer $key" }
  $endpoint = $BaseUrl.TrimEnd('/')
  if (-not $endpoint.EndsWith('/chat/completions')) { $endpoint += '/chat/completions' }
  $body = @{model=$Model; messages=@(@{role='user'; content='Reply OK'}); stream=$false} | ConvertTo-Json -Depth 5
  $reply = Invoke-RestMethod -Uri $endpoint -Method Post -Headers $headers -Body ([Text.Encoding]::UTF8.GetBytes($body)) -ContentType 'application/json' -TimeoutSec 30
  if (-not $reply.choices[0].message.content) { throw 'Gateway reply has no choices[0].message.content.' }
  Write-Output 'OpenAI-compatible non-streaming response: OK. Test streaming in the app as well.'
} catch { throw 'Model request failed. Check gateway address, model, authentication header and gateway logs.' }
finally { [System.Net.WebRequest]::DefaultWebProxy = $previousProxy; [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer); $key=$null; $headers=$null; $secret.Dispose() }
