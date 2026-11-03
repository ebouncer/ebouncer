# eBouncer

> The OS-level bouncer for AI coding agents. —— 给你的 AI 编码代理请一位操作系统级的保安。

**状态：v0.1 早期开发（dry-run 阶段）。** 当前能力：跨工具 AI 花费账本（ledger）、按客户归因出账（bill）、危险命令 dry-run 观察。拦截、内核探针、Shadow Vault 恢复在路线图上（见 ROADMAP）。

## 为什么（Why now）

- 2026-08：用户要求"先备份再改"，Claude 备份进错目录后强删原件——**48,000 个文件、103 秒，连本地 Git 恢复记录一起没了**。
- 2026-04：PocketOS 的 agent **9 秒**删掉生产数据库卷（含备份）。
- 2025-12：`rm -rf ... ~/`——Claude 主目录清空。
- Anthropic 自己的测试：人类只拦得住 13.6% 的危险命令，自动分类器 89%——**拦不住是常态**。
- 2026-11-01 起：DeepSeek Search/Fetch 按次计费——AI 账单从"纯 token"变成"token + 工具按次"，**没人替你记这本账**。

Claude Code / Codex 的防护停在宿主工具层，且可以一键跳过（`EVEN_MORE_DANGEROUSLY_SKIP_PERMISSIONS`）。eBouncer 做三件宿主做不到的事：**跨工具合并账本、按客户开票、命令级留痕与拦截**。

## 快速开始（v0.1，Linux / WSL）

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
# 或从源码：cargo install --path crates/ebouncer-cli

ebc demo            # 生成演示账本
ebc ledger --days 7 # 看：工具 × 项目 × 客户 的花费矩阵
ebc ingest          # 扫宿主日志（DSH/Claude Code/Codex），记第一笔真账
ebc bill --client <客户名> --format pdf --out   # 给甲方开一张带明细的账单
ebc doctor          # 环境自检：宿主日志在不在、钩子注入状态、审计链健康
```

## 数据都在你自己的机器上

`~/.ebouncer/`（0700），纯本地 JSONL。审计链哈希可自证完整（`ebc audit-verify`）；命令中的密钥在**落盘前**替换为哈希引用，明文永不进日志（PRIVACY.md 有字段白名单）。

## 参与

- **Beta 招募中（30 名）**：Linux/WSL/Git-Bash/MSYS2 优先，[5 分钟上手清单见 BETA.md](BETA.md)
- 误拦/漏拦/兼容问题走 issue 模板（结构化勾选），反馈直接进规则库
- 安全问题先走 [SECURITY.md](SECURITY.md) 的私下披露

## License

AGPL-3.0-only（开源核心永久免费；商用版/托管版另行授权）。
