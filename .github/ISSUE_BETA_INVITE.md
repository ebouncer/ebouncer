装 v0.1.0-rc.7，跑三件事，换一张客户能自己验证的账单。

## 1. 安装（30 秒）

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
ebc --version        # 应显示 0.1.0（装到的 tag 见输出）
# 需要固定版本时：
# EBC_VERSION=v0.1.0-rc.7 sh install.sh
```

安装失败请贴：`uname -a`、`ebc --version`、`ebc doctor` 原始输出。

## 2. 跑三件事（截图或贴文本都行）

```bash
ebc                                              # 向导：宿主 + 近 30 天花费
ebc tag 测试甲方 && ebc bill --client 测试甲方    # table 即可，不强制 PDF
ebc bundle --client 测试甲方
ebc verify-bundle ~/.ebouncer/exports/*.ebc.zip  # 预期：✅ 证据包完整 …
```

第三件是这一版的重点：生成的 `.ebc.zip` 可以发给客户，对方解压后 `sha256sum -c SHA256SUMS` 能验文件没被改；装了 eBouncer 的一方还能用包内账本和费率**重算金额**。

## 3. 回报（一条评论即可）

- 环境：Linux / WSL / Git-Bash + bash/zsh
- `ebc --version`
- 三件事的终端输出（项目名、路径、客户名可打码）
- 一句话：数字和你直觉差在哪？哪一步卡住了？
- 误拦 / 漏拦 / 兼容问题 → 走 `beta-feedback` issue 模板

## 已知不算 bug（v0.1.0-rc.7）

- Claude 行按 **USD（$）** 计，DeepSeek 按 **CNY（¥）** 计；同一客户跨币种会被明确拒绝（汇率层未做）。
- shell 钩子只覆盖**交互式终端**；agent 的 `bash -c` 不经过（宿主原生 hook 适配在 v0.2）。
- bash 下钩子提示延迟到下一提示符（zsh 是即时的）。
- `--format pdf` 找不到 Chrome/Edge 时会降级为 HTML（Linux/WSL 常见）。
- Windows 原生钩子未支持（WSL 内可用）。
- 空账本时 `ebc ledger` 退出 1 是预期。

## 数据边界

全部本地，无云端。issue 是公开的，贴之前把项目名、路径、客户名和密钥打码。
