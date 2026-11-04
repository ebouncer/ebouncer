#!/usr/bin/env bash
# eBouncer 安装脚本 —— 纯用户态：不碰 root、不替换 shell、只写 ~/.local/bin 与 ~/.ebouncer
# 用法：curl -fsSL https://ebouncer.dev/install.sh | sh
#      EBC_VERSION=v0.1.0 curl ... | sh   # 固定版本
#      EBC_CHANNEL=rc curl ... | sh       # 最新预发布（auto 已含 rc；stable 只取稳定版）
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

fetch_tag() { curl -fsSL --retry 5 --retry-delay 2 --retry-all-errors "$1" | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4; }

if [ -z "${EBC_VERSION:-}" ]; then
  case "${EBC_CHANNEL:-auto}" in
    stable)
      # 只取稳定版（GitHub 的 latest 不含 prerelease）
      EBC_VERSION=$(fetch_tag "https://api.github.com/repos/$REPO/releases/latest") || true
      ;;
    *)
      # auto / rc：取最新一条 release（含预发布）。
      # v0.1.0 正式发布前没有稳定版，默认必须能装到 rc，否则会拿到旧版。
      EBC_VERSION=$(fetch_tag "https://api.github.com/repos/$REPO/releases?per_page=10") || true
      ;;
  esac
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
curl -fsSL --retry 6 --retry-delay 3 --retry-all-errors -o "$tmp/$asset" "$base/$asset"
if curl -fsSL --retry 3 --retry-delay 2 --retry-all-errors -o "$tmp/checksums.txt" "$base/checksums.txt" 2>/dev/null; then
  ( cd "$tmp" && sha256sum -c --ignore-missing checksums.txt ) || {
    echo "校验失败：产物与 checksums.txt 不符，中止安装" >&2; exit 1; }
fi

mkdir -p "$BIN_DIR"
# MSYS/Git-Bash 下把目标目录导出为原生 Windows 路径再交给 tar/unzip，
# 避免 POSIX 形如 /c/... 的路径被 MSYS 自动转换破坏（Linux 下 cygpath 不存在则原样使用）。
to_native() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi
}
pkg_tmp="$tmp/pkg"
mkdir -p "$pkg_tmp"
case "$target" in
  *-musl)
    tar -xzf "$tmp/$asset" -C "$pkg_tmp"
    mv "$pkg_tmp/ebc" "$BIN_DIR/ebc" ;;
  *)
    unzip -o "$tmp/$asset" -d "$(to_native "$pkg_tmp")" >/dev/null
    mv "$pkg_tmp/ebc.exe" "$BIN_DIR/ebc.exe" ;;
esac
chmod +x "$BIN_DIR/ebc" 2>/dev/null || true

echo "✅ 已安装 $BIN_DIR/ebc（$EBC_VERSION）"
if [ "${EBC_NO_PATH:-0}" != "1" ]; then
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
      # 用户态安装目录默认不在 PATH：写入 shell rc（幂等，可 EBC_NO_PATH=1 跳过）
      for rc in "$HOME/.profile" "$HOME/.bashrc"; do
        [ -f "$rc" ] || continue
        grep -q 'eBouncer PATH' "$rc" && continue
        {
          printf '\n# >>> eBouncer PATH >>>\n'
          printf 'export PATH="%s:$PATH"\n' "$BIN_DIR"
          printf '# <<< eBouncer PATH <<<\n'
        } >> "$rc"
      done
      echo "已把 $BIN_DIR 加入 PATH（重开终端生效；EBC_NO_PATH=1 可跳过）"
      ;;
  esac
fi
echo "下一步：ebc doctor（自检）→ ebc ingest（记第一笔账）"
