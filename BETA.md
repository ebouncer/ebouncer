# eBouncer Beta 上手（3 分钟版）

> 你只需要 3 条命令。每一步都有"预期"，对不上就是 bug——按 issue 模板提单，比什么新功能都值钱。
> 逃生门：命令尾加 `#ebc:allow` 单次豁免；`EBC_DISABLE=1` 整体临时停用；`ebc uninstall` 随时摘钩子（账本与审计链保留）。

## 1 分钟装好

```bash
curl -fsSL https://raw.githubusercontent.com/ebouncer/ebouncer/main/install.sh | sh
```

**预期**：`✅ 已安装 .../ebc`。若提示 PATH，就照提示补一行 `export`（新开终端生效）。

## 2 分钟看到自己的账

```bash
ebc
```

就这一条。无参数的 `ebc` 是向导：自动发现你装过的 AI 编码工具（DSH / Claude Code / Codex）→ 自动把本地会话日志记成账 → 直接给"近 30 天花费矩阵 + 最烧钱的客户/项目"和下一步建议。

**预期**：看到自己的真实 token 数与 ≈¥ 金额。第一次跑如果日志是旧的格式，向导会提示 `ebc rebuild`（一键重建，旧账本自动改名保留，不会丢）。

## 30 秒开一张票（可选，但请试）

```bash
cd 你的某个项目目录
ebc tag 测试甲方        # 挂客户名；存量账目自动重新归因
ebc bill                # 不带参数：列出可开票对象 + 下一行可复制命令
ebc bill --client 测试甲方 --format pdf --out
```

**预期**：`~/.ebouncer/exports/` 里出现 PDF（明细 + 合计 + 账本指纹 + 斜排水印）。前 3 张加 `--trial-clean` 无水印。

## 4 分钟挂上哨兵（可选）

```bash
ebc init         # 自动探测 shell，只预览要注入什么
ebc init --yes   # 确认后注入；重开终端生效
```

之后你在**交互式终端**里敲的每条命令经过 30 条危险规则基线：命中就提示并记入审计链（**只提示，不拦截**——v0.1 是观察期）。注意：agent 自己用 `bash -c` 执行的命令**不经过** shell 钩子（宿主原生 hook 适配在 v0.2）。`ebc doctor` 随时看状态，`ebc audit-verify` 校验哈希链 + 尾部 checkpoint（能发现断链与尾部截断）。

## beta 任务（二选一即可）

- **A**：正常用终端 3~5 天。任何"不该弹的弹了"（误拦）、"vim/tmux/sudo 不对劲"（兼容）、"数字和我直觉差太多"（账目），走 issue 模板；
- **B**：真的用 `ebc bill --format pdf` 给甲方/自己开一张票，把感受发 issue #1。

误报数据是规则库的命根子：Anthropic 的数据显示人类只拦得住 13.6% 的危险命令，所以拦截上线前，我们必须先把误报磨到接近 0。
