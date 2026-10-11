# Verify a transferred kit before first installation; no Python or network needed.
[CmdletBinding()]
param([switch]$ToolsOnly)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$manifest = if ($ToolsOnly) { 'TOOLS-SHA256SUMS.txt' } else { 'SHA256SUMS.txt' }
$file = Join-Path $root $manifest
if (-not (Test-Path $file)) { throw "Missing checksum manifest: $manifest" }
$count = 0
foreach ($line in [IO.File]::ReadLines($file)) {
  if ($line -notmatch '^([a-f0-9]{64})  (.+)$') { throw 'Malformed checksum manifest.' }
  $expected, $name = $matches[1], $matches[2]
  # Portable editor settings are deliberately user-editable after first use.
  if ($ToolsOnly -and $name.StartsWith('tools/editor/data/')) { continue }
  $path = [IO.Path]::GetFullPath((Join-Path $root $name))
  if (-not $path.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Checksum path escapes the kit.' }
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing kit file: $name" }
  if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $expected) { throw "Damaged kit file: $name" }
  $count++
}
Write-Output "Kit integrity verified: $count files."
