#!/usr/bin/env bash
# eBouncer 安装脚本 —— 纯用户态：不碰 root、不替换 shell、只写 ~/.local/bin 与 ~/.ebouncer
# 用法：curl -fsSL https://ebouncer.dev/install.sh | sh
#      EBC_VERSION=v0.1.0 curl ... | sh   # 固定版本
set -eu

REPO="ebouncer/ebouncer"
BIN_DIR="${EBC_BIN_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-musl" ;;
      *) echo "暂不支持的架构: $arch（aarch64 进 v0.2）" >&2; exit 1 ;;
    esac ;;
  MINGW*|MSYS*|CYGWIN*) target="x86_64-pc-windows-msvc" ;;
  Darwin) echo "macOS 支持在 v0.5 提供；Linux/WSL 用户可先装。" >&2; exit 1 ;;
  *) echo "不支持的系统: $os" >&2; exit 1 ;;
esac

if [ -z "${EBC_VERSION:-}" ]; then
  EBC_VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4) || true
fi
[ -n "${EBC_VERSION:-}" ] || { echo "无法解析最新版本号，请设 EBC_VERSION（如 v0.1.0）" >&2; exit 1; }

case "$target" in
  *-musl) asset="ebouncer-$EBC_VERSION-$target.tar.gz" ;;
  *)      asset="ebouncer-$EBC_VERSION-$target.zip" ;;
esac
base="https://github.com/$REPO/releases/download/$EBC_VERSION"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "下载 $asset ..."
curl -fsSL -o "$tmp/$asset" "$base/$asset"
if curl -fsSL -o "$tmp/checksums.txt" "$base/checksums.txt" 2>/dev/null; then
  ( cd "$tmp" && sha256sum -c --ignore-missing checksums.txt ) || {
    echo "校验失败：产物与 checksums.txt 不符，中止安装" >&2; exit 1; }
fi

mkdir -p "$BIN_DIR"
case "$target" in
  *-musl)
    tar -xzf "$tmp/$asset" -C "$tmp"
    mv "$tmp/ebc" "$BIN_DIR/ebc" ;;
  *)
    unzip -o "$tmp/$asset" -d "$tmp" >/dev/null
    mv "$tmp/ebc.exe" "$BIN_DIR/ebc.exe" ;;
esac
chmod +x "$BIN_DIR/ebc" 2>/dev/null || true

echo "✅ 已安装 $BIN_DIR/ebc（$EBC_VERSION）"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "提示：把 $BIN_DIR 加入 PATH，例如 echo 'export PATH=\"\$PATH:$BIN_DIR\"' >> ~/.bashrc && source ~/.bashrc" ;;
esac
echo "下一步：ebc doctor（自检）→ ebc ingest（记第一笔账）"
