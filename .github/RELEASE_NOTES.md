## 安装

**Linux / WSL**

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
```

**Windows（PowerShell 5.1+，无需管理员）**

```powershell
irm https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.ps1 | iex
```

装完**重开终端**，`ebc --version` 验证。PATH 排错、升级/卸载见 [INSTALL.md](https://github.com/ebouncer/ebouncer/blob/main/INSTALL.md)。

## 这是什么

跨工具 AI 花费账本 + 按客户出可验证成本账单：

```bash
ebc                                      # 记账 + 看花费
ebc tag 客户名 --dir /path/to/project     # 归因（只开票才需要）
ebc bill --client "客户名" --pdf          # 出成本账单 PDF
```

- 默认**无水印**；成本核算不要加 `--markup`（`--markup` 是报价单）
- 加 `--bundle` 同时出证据包；客户可 `verify-bundle` 离线复算
- 数据全部在本地，不上传云端

## 已知限制（v0.1）

- 只提供 Linux x86_64 与 Windows x86_64；arm64 在 v0.2
- 单币种账单；同一客户跨币种会明确拒绝
- Claude Code / Codex 适配器就绪，仍待更多真实样本回归
- shell 钩子只覆盖**交互式终端**；agent 的 `bash -c` 不经过
- 找不到 Edge/Chrome 时 PDF 会降级为 HTML

## Beta 反馈

把 `ebc --version`、系统版本、终端输出（项目名/路径/客户名打码）贴到：

https://github.com/ebouncer/ebouncer/issues/1
