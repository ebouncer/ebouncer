# eBouncer Beta 上手清单（5 分钟版）

> 给 beta 用户的唯一文档。原则：每一步都有"预期输出"，对不上就是 bug，直接提 issue。
> 两个逃生门先记住：命令尾加 `#ebc:allow` 单次豁免；`EBC_DISABLE=1` 临时整体停用。

## 0. 你适合参加吗

- [ ] Linux / WSL / Git-Bash / MSYS2（Windows 原生暂不支持）
- [ ] 至少用过 DSH、Claude Code、Codex 其中一个（有真实会话日志才有账可记）
- [ ] 愿意每周提 ≥1 条 issue（误拦/漏拦/兼容都算，哪怕"没有"）

## 1. 安装（30 秒）

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
export PATH="$PATH:$HOME/.local/bin"   # 若提示找不到 ebc
ebc --version                          # 预期：ebc 0.1.0-rc.2
```

## 2. 先看演示，再记真账（1 分钟）

```bash
ebc demo && ebc ledger --days 7
```
预期：三行演示账本（claude-code/codex/dsh），codex 那行 ≈¥17.20。这一步确认安装完好。

```bash
ebc ingest
```
预期：`已入账 N 条事件`（N>0 说明扫到了你的真实会话；N=0 且你确实用过上述工具 → 提 issue，把你机器上 `~/.dsh` 或 `~/.claude/projects` 是否存在写清楚）。

```bash
ebc ledger --days 30
```
预期：你的真实花费矩阵。重点核对：token 量级和你直觉是否一致、`search*`/`fetch*` 列是否抓到了你的联网调用。

## 3. 开一张你自己的账单（1 分钟）

```bash
mkdir -p <你的项目目录>/.ebouncer && echo "测试客户" > <你的项目目录>/.ebouncer/client
ebc ingest          # 重新入账才会带上客户标签（归因发生在入账时刻）
ebc bill --client "测试客户" --days 30 --format pdf --out
```
预期：`~/.ebouncer/exports/invoice-*.pdf`，打开能看到明细表 + 斜排水印。
**beta 任务①**：把这张 PDF（金额随意）发到 issue #1 评论区，让我们看看"发给甲方的第一张票"长什么样。

## 4. 开启命令观察（1 分钟）

```bash
ebc init --shell zsh        # 或 bash：先看预览
ebc init --shell zsh --yes  # 确认后注入
# 重开终端
```
测试：
```bash
mkdir -p /tmp/ebc-test && git init -q /tmp/ebc-test && cd /tmp/ebc-test
git clean -fd
```
预期：命令照常执行，出现 `⚠️ eBouncer(dry-run) 命中 log-ask [git-clean-f]` 提示（bash 下延迟一拍到下一提示符属正常）。
```bash
ebc doctor && ebc audit-verify
```
预期：钩子"已注入"、审计链"完好"。

**beta 任务②**：正常用你的终端 3~5 天（vim/tmux/ssh/sudo/构建命令都照常跑）。任何"不该弹的弹了"（误拦）或"交互程序不对劲"（兼容），按 issue 模板提单——这类数据是规则库和兼容矩阵的原料，比新功能值钱。

## 5. 卸载与数据

```bash
ebc uninstall          # 只摘 rc 钩子；账本/审计链/导出全部保留
ebc uninstall --purge  # 连可再生状态一起清（ledger/exports 仍在）
```
真删证据需要你自己 `rm -rf ~/.ebouncer`——故意不给开关，卸载不应销毁审计。

## 反馈约定

- 一切问题走 issue 模板（误拦/漏拦/兼容/数字不对/安装，结构化勾选）；
- 贴命令前确认无密钥（eBouncer 本地自动脱敏，但 issue 是公开的）；
- 每周三我发一次"本周规则库变更"，你的误拦报告被修掉会 @ 你。
