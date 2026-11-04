装 v0.1.0-rc.11，跑三件事，换一张客户能自己验证的账单。

## 1. 安装（30 秒）

**Linux / WSL**

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
```

**Windows（PowerShell 5.1+，无需管理员）**

```powershell
irm https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.ps1 | iex
```

装完**重开终端**，验证：

```bash
ebc --version        # 0.1.0
ebc bill --help      # 应看到 --pdf / --bundle
```

固定版本：`EBC_VERSION=v0.1.0-rc.11`（Linux）或 `$env:EBC_VERSION="v0.1.0-rc.11"`（Windows）。

安装失败请贴：`uname -a`（Windows 贴 `$PSVersionTable`）、`where.exe ebc`、`ebc --version`、`ebc doctor` 原始输出。

## 2. 跑三件事（截图或贴文本都行）

```bash
ebc                                              # 向导：宿主 + 近 30 天花费
cd 你的项目 && ebc tag 测试甲方
ebc bill --client 测试甲方 --pdf                 # 成本账单，默认无水印
ebc bill --client 测试甲方 --pdf --bundle        # 再出一份可验证证据包
ebc verify-bundle ~/.ebouncer/exports/*.ebc.zip  # 预期：✅ 证据包完整 …
```

第三件是这一版的重点：生成的 `.ebc.zip` 可以发给客户，对方解压后 `sha256sum -c SHA256SUMS` 能验文件没被改；装了 eBouncer 的一方还能用包内账本和费率**重算金额**。

> 成本核算**不要加 `--markup`**；只有报价场景才用 `--markup 0.2`，标题会变成「AI 用量报价单」。

## 3. 回报（一条评论即可）

- 环境：Linux / WSL / Windows（PowerShell / Git-Bash）+ 版本
- `ebc --version`、`where.exe ebc`（Windows）或 `which ebc`（Linux）
- 三件事的终端输出（项目名、路径、客户名可打码）
- 一句话：数字和你直觉差在哪？哪一步卡住了？
- 误拦 / 漏拦 / 兼容问题 → 走 `beta-feedback` issue 模板

## 已知不算 bug（v0.1.0-rc.11）

- 默认无水印；`--watermark` 只用于演示/截图，`--trial-clean` 保留但无作用。
- Claude 行按 **USD（$）** 计，DeepSeek 按 **CNY（¥）** 计；同一客户跨币种会被明确拒绝（汇率层未做）。
- Search/Fetch 从 **2026-11-01** 起才计费；更早的事件只记量、金额 0。
- shell 钩子只覆盖**交互式终端**；agent 的 `bash -c` 不经过（宿主原生 hook 适配在 v0.2）。
- bash 下钩子提示延迟到下一提示符（zsh 是即时的）。
- 找不到 Chrome/Edge 时会降级为 HTML（Linux/WSL 常见）。
- Windows 原生钩子未支持（WSL 内可用）。
- 空账本时 `ebc ledger` 退出 1 是预期。
- 只提供 Linux x86_64 与 Windows x86_64；arm64 在 v0.2。

## 数据边界

全部本地，无云端。issue 是公开的，贴之前把项目名、路径、客户名和密钥打码。
