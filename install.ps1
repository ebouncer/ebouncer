#Requires -Version 5.1
<#
.SYNOPSIS
  eBouncer Windows 安装脚本（用户态，无需管理员）。
.DESCRIPTION
  下载最新 release 的 Windows 构建，校验 sha256，安装到
  %LOCALAPPDATA%\eBouncer\bin，并把该目录加入当前用户 PATH。
  用法：
    irm https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.ps1 | iex
  固定版本：
    $env:EBC_VERSION = "v0.1.0-rc.8"; irm ... | iex
  不改 PATH：
    $env:EBC_NO_PATH = "1"; irm ... | iex
#>
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
    if (-not $releases -or $releases.Count -eq 0) { throw "无法获取最新版本，请设置 EBC_VERSION" }
    $version = $releases[0].tag_name
  }

  $asset = "ebouncer-$version-x86_64-pc-windows-msvc.zip"
  $base = "https://github.com/$repo/releases/download/$version"
  Write-Host "下载 $asset ..."
  Invoke-WebRequest -Uri "$base/$asset" -OutFile (Join-Path $tmp $asset) -UseBasicParsing
  Invoke-WebRequest -Uri "$base/checksums.txt" -OutFile (Join-Path $tmp "checksums.txt") -UseBasicParsing

  $line = Get-Content (Join-Path $tmp "checksums.txt") |
    Where-Object { $_ -match [regex]::Escape($asset) } |
    Select-Object -First 1
  if (-not $line) { throw "checksums.txt 中找不到 $asset" }
  $expected = ($line -split '\s+')[0].ToLower()
  $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $expected) { throw "sha256 校验失败：期望 $expected，实际 $actual" }

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
  Write-Host "✅ 已安装：$binDir\ebc.exe"
  Write-Host "新开一个终端后运行：ebc"
  if ($noPath) { Write-Host "（EBC_NO_PATH=1：未修改 PATH，请手动加入 $binDir）" }
} finally {
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
