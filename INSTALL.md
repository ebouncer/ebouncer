# 安装与开票（3 分钟）

eBouncer 是纯本地工具：装到用户目录、不需要管理员、不联网上传数据。

## 1. 安装

### Linux / WSL（x86_64）

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
```

- 装到 `~/.local/bin/ebc`，并自动把该目录写进 `~/.profile` / `~/.bashrc` 的 PATH。
- 重开终端，验证：

```bash
ebc --version      # 应显示 0.1.0
ebc bundle --help  # 能打印帮助 = 新版
```

- 不想改 PATH：`EBC_NO_PATH=1 curl ... | sh`
- 固定版本：`EBC_VERSION=v0.1.0-rc.10 curl ... | sh`
- 只支持 x86_64；arm64 在 v0.2。

### Windows（PowerShell 5.1+，无需管理员）

```powershell
irm https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.ps1 | iex
```

- 装到 `%LOCALAPPDATA%\eBouncer\bin\ebc.exe`，并把该目录加入**当前用户** PATH。
- 新开一个 PowerShell 窗口，验证：

```powershell
ebc --version
ebc bundle --help
```

- 固定版本：`$env:EBC_VERSION="v0.1.0-rc.10"; irm ... | iex`
- 不改 PATH：`$env:EBC_NO_PATH="1"; irm ... | iex`
- Windows 需要 Edge 或 Chrome 才能出 PDF；没有时自动降级为 HTML。

### Windows + Git-Bash（备选）

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
```

装到 `~/.local/bin/ebc.exe`；PowerShell/cmd 里使用时仍需把该目录加入 PATH，或在 Git-Bash 里用。

## 2. 记第一笔账

```bash
ebc
```

向导会自动发现 DSH / Claude Code / Codex 的日志 → 自动入账 → 打印近 30 天花费矩阵。

## 3. 挂客户 + 开票

```bash
cd 你的项目目录
ebc tag 测试甲方          # 把该项目归因到客户（存量账目一起改）

ebc bill                  # 列出可开票客户
ebc bill --client "测试甲方" --pdf            # 出 PDF（成本口径）
ebc bill --client "测试甲方" --pdf --bundle   # PDF + 可验证证据包
```

- 文件在：
  - Linux/WSL：`~/.ebouncer/exports/`
  - Windows：`%USERPROFILE%\.ebouncer\exports\`
- 导出永不覆盖；同名自动加 `-2`、`-3`。
- **成本核算不要加 `--markup`**；`--markup 0.2` 是报价单（含 20% 服务费），标题会变成「AI 用量报价单」。
- 默认无水印；`--watermark` 只用于演示/截图。`--trial-clean` 保留为兼容参数，已无作用。
- 同一客户跨币种（CNY + USD）会被明确拒绝；v0.1 不做汇率换算。

## 4. 常用命令

| 目的 | 命令 |
|---|---|
| 看自己的全部费用 | `ebc ledger --days 365` |
| 给项目挂客户 | `ebc tag 客户名` |
| 看有哪些客户可开票 | `ebc bill` |
| 出成本账单 PDF | `ebc bill --client "客户名" --pdf` |
| 账单 + 证据包 | `ebc bill --client "客户名" --pdf --bundle` |
| 只出证据包 | `ebc bundle --client "客户名"` |
| 验证客户发回的包 | `ebc verify-bundle 包.ebc.zip` |
| 环境自检 | `ebc doctor` |
| 审计链校验 | `ebc audit-verify` |

## 5. 升级 / 卸载

**升级**：重跑安装命令即可，账本与配置保留。

**卸载**：

```bash
ebc uninstall        # 摘掉 shell 钩子；账本与审计链保留
```

然后按需删除：

- Linux：`~/.local/bin/ebc`、`~/.ebouncer/`
- Windows：`%LOCALAPPDATA%\eBouncer\`、`%USERPROFILE%\.ebouncer\`，并从用户 PATH 移除对应目录。

## 6. 常见问题

| 现象 | 处理 |
|---|---|
| `ebc: command not found` | 重开终端；或 `export PATH="$HOME/.local/bin:$PATH"` / 把 `%LOCALAPPDATA%\eBouncer\bin` 加入 PATH |
| `ebc bundle` 不认识 | 装到了旧版：重跑安装命令，`where ebc` 确认指向新版 |
| PDF 没生成 | 装 Edge/Chrome；或先用 `--format csv` / HTML |
| 金额是 0 | 该模型没有费率条目；`ebc doctor` 看，或等费率表更新 |
| 金额和直觉差很多 | 先看 `ebc ledger` 的价档（高峰/空闲）和 `--days` 窗口 |
| 客户跨币种报错 | 按币种拆客户（v0.1 不做汇率） |
| 水印去不掉 | 正式版默认无水印；检查是否显式传了 `--watermark` |

## 7. 数据与隐私

- 全部在本地：账本、费率表、审计链，无云端上传。
- 命令里的密钥在**落盘前**替换为哈希引用；明文不进日志。
- 证据包默认不含审计链；`--with-audit` 才带（命令文本已脱敏）。
- 更多见 [PRIVACY.md](PRIVACY.md) 与 [SECURITY.md](SECURITY.md)。
