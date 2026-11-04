#!/usr/bin/env bash
# v0.1.0 自动验收（本机 / CI 通用）。
#
# 覆盖 RELEASING.md 的 G1–G6 可自动化部分：
#   fmt / clippy / tests（--fast 跳过）、构建、版本一致、demo、空账本、
#   审计截断检测、混币拒绝、证据包生成/验证/篡改检测、install.sh 语法。
#
# 用法：
#   bash scripts/release-check.sh          # 完整（发版前跑）
#   bash scripts/release-check.sh --fast   # 跳过 lint/test（CI 行为门用）
#
# G7（发布资产）与 G8（外部用户安装）仍需人工确认。
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 1

FAST=0
[ "${1:-}" = "--fast" ] && FAST=1

fail=0
ok() { printf '  PASS  %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; fail=1; }
section() { printf '\n== %s ==\n' "$1"; }

if [ "$FAST" -eq 0 ]; then
  section "lint / test"
  if cargo fmt --all -- --check >/dev/null 2>&1; then
    ok "cargo fmt --check"
  else
    bad "cargo fmt --check（先跑 cargo fmt --all）"
  fi
  if cargo clippy --workspace --all-targets -- -D warnings >/dev/null 2>&1; then
    ok "cargo clippy -D warnings"
  else
    bad "cargo clippy -D warnings"
  fi
  if cargo test --workspace >/dev/null 2>&1; then
    ok "cargo test"
  else
    bad "cargo test"
  fi
fi

section "构建"
if cargo build -q -p ebouncer-cli; then
  ok "cargo build -p ebouncer-cli"
else
  bad "cargo build"
  exit 1
fi
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
if [ -x "$TARGET/debug/ebc" ]; then
  EBC="$TARGET/debug/ebc"
elif [ -x "$TARGET/debug/ebc.exe" ]; then
  # Git-Bash / MSYS2
  EBC="$TARGET/debug/ebc.exe"
else
  bad "找不到 target/debug/ebc（Linux）或 ebc.exe（Windows）"
  exit 1
fi

section "版本一致性"
ver=$("$EBC" --version | awk '{print $2}')
if grep -q "^version = \"$ver\"" Cargo.toml; then
  ok "Cargo.toml 版本 = $ver"
else
  bad "Cargo.toml 版本与 ebc --version（$ver）不一致"
fi
if grep -q "## \[$ver\]" CHANGELOG.md; then
  ok "CHANGELOG 有 [$ver]"
else
  bad "CHANGELOG 缺 [$ver]"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

section "demo"
if "$EBC" demo >/dev/null 2>&1; then ok "ebc demo"; else bad "ebc demo"; fi

section "空账本（预期退出 1）"
"$EBC" ledger --days 7 --data-dir "$tmp/empty" >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 1 ]; then
  ok "空账本退出 1"
else
  bad "空账本退出码 $rc（应为 1）"
fi

section "审计链截断检测"
adir="$tmp/audit"
mkdir -p "$adir"
printf 'rm -rf ~\n' | "$EBC" hook --data-dir "$adir" >/dev/null 2>&1
printf 'git push --force\n' | "$EBC" hook --data-dir "$adir" >/dev/null 2>&1
if "$EBC" audit-verify --data-dir "$adir" >/dev/null 2>&1; then
  ok "审计链完好"
else
  bad "审计链应完好却报错"
fi
# 去掉最后一行（保留 checkpoint），必须被检出
awk 'NR > 1 { print prev } { prev = $0 }' "$adir/audit.jsonl" > "$adir/audit.tmp"
mv "$adir/audit.tmp" "$adir/audit.jsonl"
if "$EBC" audit-verify --data-dir "$adir" >/dev/null 2>&1; then
  bad "删掉尾部记录后仍报完好"
else
  ok "尾部截断被检出"
fi

section "混币拒绝"
mdir="$tmp/mix"
mkdir -p "$mdir"
cat > "$mdir/ledger.jsonl" <<'JSONL'
{"ts":"2026-11-03T02:00:00Z","session_id":"c1","tool":"claude-code","project":"p","client":"MIX","model":"claude-sonnet-5-5","tokens_in":1000,"currency":"USD"}
{"ts":"2026-11-03T02:00:00Z","session_id":"d1","tool":"dsh","project":"p","client":"MIX","model":"deepseek-flash","tokens_in":1000,"currency":"CNY"}
JSONL
"$EBC" bill --client MIX --days 30 --data-dir "$mdir" >/dev/null 2>"$mdir/err"
rc=$?
if [ "$rc" -ne 0 ] && grep -q "跨币种" "$mdir/err"; then
  ok "跨币种账单被明确拒绝"
else
  bad "跨币种账单未按预期拒绝（exit=$rc）"
fi

section "证据包生成 / 验证 / 篡改检测"
bdir="$tmp/bundle"
mkdir -p "$bdir"
cat > "$bdir/ledger.jsonl" <<'JSONL'
{"ts":"2026-11-03T02:00:00Z","session_id":"s1","tool":"dsh","project":"p","client":"ACME","model":"deepseek-flash","tokens_in":1000000,"tokens_out":0,"currency":"CNY"}
JSONL
if "$EBC" bundle --client ACME --days 30 --data-dir "$bdir" >/dev/null 2>&1; then
  ok "ebc bundle"
else
  bad "ebc bundle"
fi
zip=$(find "$bdir/exports" -name '*.ebc.zip' -print -quit 2>/dev/null)
if [ -n "$zip" ] && "$EBC" verify-bundle "$zip" >/dev/null 2>&1; then
  ok "ebc verify-bundle"
else
  bad "ebc verify-bundle"
fi
if [ -n "$zip" ] && command -v python3 >/dev/null 2>&1; then
  python3 - "$zip" "$tmp/tampered.ebc.zip" <<'PY'
import sys
data = bytearray(open(sys.argv[1], "rb").read())
i = data.find(b"ACME")
if i < 0:
    raise SystemExit("payload not found")
data[i] ^= 0x01
open(sys.argv[2], "wb").write(data)
PY
  if "$EBC" verify-bundle "$tmp/tampered.ebc.zip" >/dev/null 2>&1; then
    bad "篡改一个字节后仍通过验证"
  else
    ok "篡改被检出"
  fi
else
  printf '  SKIP  篡改检测（无 python3）\n'
fi

section "安装脚本语法"
if bash -n install.sh; then ok "bash -n install.sh"; else bad "install.sh 语法"; fi

printf '\n'
if [ "$fail" -eq 0 ]; then
  echo "✅ release-check 全部通过（G7/G8 仍需人工确认，见 RELEASING.md）"
  exit 0
else
  echo "❌ release-check 有失败项（见上）"
  exit 1
fi
