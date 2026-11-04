# eBouncer

> 跨工具的 AI 花费账本与审计证据层。 —— 你雇了三个 AI 代理，得有一本共同的账。

**状态：v0.1.0-rc.10 beta。** 当前能力：跨工具 AI 花费账本（ledger）、按客户归因出成本账单（bill）、可携带证据包（bundle / verify-bundle）、危险命令 dry-run 观察（仅交互式 shell）。计价覆盖 DeepSeek（CNY）与 Anthropic（USD）的单币种账单；混币客户暂不支持。宿主原生 hook 适配、内核探针、Shadow Vault 恢复在路线图上（见 ROADMAP）。

## 快速开始

**Linux / WSL**

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
```

**Windows（PowerShell 5.1+）**

```powershell
irm https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.ps1 | iex
```

装完重开终端，`ebc --version` 验证；PATH 排错、升级/卸载见 [INSTALL.md](INSTALL.md)。

**三步开票**

```bash
ebc                              # 1) 自动记账 + 近 30 天花费
cd 项目目录 && ebc tag 客户名     # 2) 归因（已归因可跳过）
ebc bill --client "客户名" --pdf  # 3) 出成本账单 PDF
```

加 `--bundle` 同时生成可验证证据包：

```bash
ebc bill --client "客户名" --pdf --bundle
```

- `ebc bill` 无参数 = 列出可开票客户
- 成本核算**不加** `--markup`；`--markup 0.2` 是报价单（含服务费）
- 默认无水印；`--watermark` 只用于演示/截图
- 更多命令见 [INSTALL.md](INSTALL.md)，上手清单见 [BETA.md](BETA.md)

## 支持矩阵（v0.1）

| 宿主 | 记账 | 计价 | 备注 |
|---|---|---|---|
| DSH | ✅ 真实日志验证 | DeepSeek（CNY） | 高峰/空闲、Search/Fetch 按次 |
| Claude Code | ⚠️ 适配器就绪，待真实样本回归 | Anthropic（USD） | 缓存写入按 5 分钟 TTL 价 |
| Codex | ⚠️ 适配器就绪，待真实样本回归 | 暂无 OpenAI 费率（只记量） | 事件币种 USD |

空模型/未知模型：只记量与 0 元行，绝不编价。同一客户跨币种时 `ebc bill` 会明确拒绝（汇率层未做）。

## 为什么（Why now）

- 2026-08：用户要求"先备份再改"，Claude 备份进错目录后强删原件——**48,000 个文件、103 秒，连本地 Git 恢复记录一起没了**。
- 2026-04：PocketOS 的 agent **9 秒**删掉生产数据库卷（含备份）。
- 2025-12：`rm -rf ... ~/`——Claude 主目录清空。
- Anthropic 自己的测试：人类只拦得住 13.6% 的危险命令，自动分类器 89%——**拦不住是常态**。
- 2026-11-01 起：DeepSeek Search/Fetch 按次计费——AI 账单从"纯 token"变成"token + 工具按次"，**没人替你记这本账**。

Claude Code / Codex / Cursor 现在都自带 hook，能在工具调用层拦截——但每家一套配置、只管自己，用户态配置还能被 agent 改掉。eBouncer 不重复造这层拦截，而是补宿主不做的那层：**跨工具合并账本、按客户归因开票、审计证据可携带**——以及 v0.2 的"一份策略分发到多家宿主"。

> **能力边界（v0.1）**：`ebc init` 注入的是 shell 钩子，只覆盖**人在交互式终端里敲的命令**；agent 通过 `bash -c` 起的命令不经过它（宿主原生 hook 适配在 v0.2）。审计链提供"篡改可发现"（含尾部截断检测），不是"不可篡改"：同一 UID 下 agent 可以改 rc、删 checkpoint、重算哈希；真正的内核级强制在 v0.3（eBPF/LSM）。

## 数据都在你自己的机器上

`~/.ebouncer/`（0700），纯本地 JSONL。审计链 = 哈希链 + 尾部 checkpoint（`ebc audit-verify` 能发现断链、删尾部、删整文件；同 UID 可同步改写，所以是"篡改可发现"而非"不可篡改"）；命令中的密钥在**落盘前**替换为哈希引用，明文永不进日志（PRIVACY.md 有字段白名单）。

## 参与

- **Beta 招募中**：Linux / WSL / Windows（PowerShell 5.1+）优先，[5 分钟上手清单见 BETA.md](BETA.md) · [晒账单 / 反馈 #1](https://github.com/ebouncer/ebouncer/issues/1)
- 误拦/漏拦/兼容问题走 issue 模板（结构化勾选），反馈直接进规则库
- 安全问题先走 [SECURITY.md](SECURITY.md) 的私下披露

## License

AGPL-3.0-only（开源核心永久免费；商用版/托管版另行授权）。
