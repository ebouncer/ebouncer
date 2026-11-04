<#
.SYNOPSIS
  eBouncer installer for Windows (userland, no admin).
.DESCRIPTION
  Downloads the latest release, verifies sha256, installs to
  %LOCALAPPDATA%\eBouncer\bin and adds it to the current user PATH.

  NOTE: keep this file ASCII-only and BOM-free. `irm ... | iex` passes the
  raw bytes as a string; a UTF-8 BOM becomes U+FEFF and breaks the first line.
  ASCII-only also keeps direct execution working on Windows PowerShell 5.1.

  Usage:
    irm https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.ps1 | iex

  Pin a version:
    $env:EBC_VERSION = "v0.1.0-rc.9"; irm ... | iex

  Skip PATH changes:
    $env:EBC_NO_PATH = "1"; irm ... | iex
#>

if ($PSVersionTable.PSVersion.Major -lt 5) {
  throw "eBouncer requires PowerShell 5.1 or newer."
}

$ErrorActionPreference = "Stop"

$repo = "ebouncer/ebouncer"
$version = $env:EBC_VERSION
$noPath = $env:EBC_NO_PATH -eq "1"
$binDir = Join-Path $env:LOCALAPPDATA "eBouncer\bin"
$tmp = Join-Path $env:TEMP ("ebc-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
  if (-not $version) {
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases?per_page=1" `
      -Headers @{ "User-Agent" = "ebouncer-install" }
    if (-not $releases -or $releases.Count -eq 0) {
      throw "Could not resolve the latest release. Set EBC_VERSION (e.g. v0.1.0-rc.9)."
    }
    $version = $releases[0].tag_name
  }

  $asset = "ebouncer-$version-x86_64-pc-windows-msvc.zip"
  $base = "https://github.com/$repo/releases/download/$version"
  Write-Host "Downloading $asset ..."
  Invoke-WebRequest -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset) -UseBasicParsing
  Invoke-WebRequest -Uri "$base/checksums.txt" -OutFile (Join-Path $tmp "checksums.txt") -UseBasicParsing

  $line = Get-Content (Join-Path $tmp "checksums.txt") |
    Where-Object { $_ -match [regex]::Escape($asset) } |
    Select-Object -First 1
  if (-not $line) { throw "checksums.txt does not list $asset" }

  $expected = ($line -split '\s+')[0].ToLower()
  $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $expected) {
    throw "sha256 mismatch: expected $expected, got $actual"
  }

  Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath (Join-Path $tmp "pkg") -Force
  New-Item -ItemType Directory -Path $binDir -Force | Out-Null
  Copy-Item (Join-Path $tmp "pkg\ebc.exe") (Join-Path $binDir "ebc.exe") -Force

  if (-not $noPath) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($null -eq $userPath) { $userPath = "" }
    $parts = @($userPath -split ';' | Where-Object { $_ -ne "" })
    $already = $parts | Where-Object { $_.TrimEnd('\') -ieq $binDir.TrimEnd('\') }
    if (-not $already) {
      [Environment]::SetEnvironmentVariable("Path", "$binDir;$userPath", "User")
    }
    $env:Path = "$binDir;$env:Path"
  }

  & (Join-Path $binDir "ebc.exe") --version
  Write-Host ""
  Write-Host "Installed: $binDir\ebc.exe"
  Write-Host "Open a new terminal and run: ebc"
  if ($noPath) {
    Write-Host "EBC_NO_PATH=1: PATH was not modified. Add $binDir manually."
  }
} finally {
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
