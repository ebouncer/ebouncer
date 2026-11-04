# 安全策略（v0.1 → v0.3 信任基建路线图）

## 当前版本承诺
- 纯本地运行，无网络请求（费率表更新除外，后续做成签名可离线验证）。
- 数据目录 Unix 0700（Windows 由用户 ACL 控制）；二进制不带 setuid、不开 cap；probe 通道（内核 eBPF）尚未发布。
- 审计链校验：`ebc audit-verify` 校验哈希链 + 尾部 checkpoint（`audit.head`），能发现改单条、删尾部、删整文件；checkpoint 缺失时明确提示"无法检测截断"，不假装校验过。
- 以上是"篡改可发现"，**不是"不可篡改"**：checkpoint 与审计文件同目录同权限，同一 UID 的攻击者可同时改写两者并重算哈希，本机无法发现。外部锚点（独立介质/远端只写归档）在 v0.2 计划中。

## 威胁模型（谁能做什么）
| 角色 | 能力 | 本版本结果 |
|---|---|---|
| 误编辑 / 崩溃 / 误删 | 坏行、尾部截断 | 能发现（断链 / checkpoint 不一致） |
| 同一 UID 的普通进程 | 改 rc、删审计尾部、重算整链 | 截断与单条改动能发现；同时改 checkpoint + 重算 → 不能 |
| AI agent（同 UID、有文件权限） | 上述全部 + 改/删 hook 配置、`EBC_DISABLE=1` | 本版本不阻止，只能留痕（且仅覆盖人敲的命令） |
| 管理员 / root | 一切 | 不能；需要外部锚点或内核探针 |

## 钩子覆盖范围（v0.1）
- `ebc init` 注入 shell 钩子（bash `PROMPT_COMMAND` / zsh `preexec`），**只在交互式终端生效**；agent 通过 `bash -c` 执行的命令不触发。
- 规则的判定对象是命令文本：`python -c`、变量拼接、`curl | sh` 等可以绕过。规则用来降低误操作和常见危险命令，不是沙箱。
- 宿主原生 hook（Claude Code PreToolUse、Codex `[hooks.PreToolUse]`）能覆盖 agent 命令，且支持托管配置锁定；`ebc init --host ...` 适配在 v0.2。在那之前，不要把本版本当"拦截"。

## 计划中（v0.3，对应 PRD §10.2 信任基建）
- cosign 签名 + SLSA L3 provenance + 每 release 的 SBOM。
- `ebc doctor --sec` 一键验证本机安装的来源与完整性。
- eBPF/LSM 探针：改变"同 UID 可篡改"这个威胁模型。

## 已知攻击面（诚实清单）
1. shell 钩子运行于用户自身 shell——能控制该 shell 者本就能伪造审计；v0.3 的 eBPF/LSM 通道才改变此威胁模型。
2. checkpoint 与审计文件同权限，可被同步改写；暂时只能发现"只删尾部 / 只改正文"。
3. ledger 解析依赖上游日志格式，改版会导致漏记（fail-open 并告警），不会静默错记。

## 报告漏洞
GitHub Security Advisory（发布 org 后启用）。请勿公开 issue。
