# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.1.0] - 目标 2026-11-20（首个公开版本，纯免费开源）

### 新增
- **ebc ledger**：跨工具 AI 花费账本。DSH 已用真实日志验证（多帧 zstd、会话去重）；Claude Code / Codex 契约经上游源码与 issue 区核验（含 Codex token_count 重发的重复计费规避）。
- **ebc ingest**：递归扫描宿主日志目录入账，K2 优先级客户归因（显式 > 项目 `.ebouncer/client` > git remote 映射）。
- **ebc bill**：按客户出账（table/csv/json/pdf）；markup 加成、估算口径与账本指纹随单；免费版前 3 张无水印（§6.2 次数级试用）；导出永不覆盖。
- **ebc init / hook / uninstall**：shell 命令防火墙（dry-run）。zsh preexec / bash PROMPT_COMMAND 注入（预览确认制、BEGIN/END 幂等块）；30 条危险命令出厂基线；命中命令落盘前脱敏（D3）写入 O(1) 哈希审计链；任何内部失败静默放行（fail-open）；卸载保留证据链。
- **ebc doctor / audit-verify / demo / rules-check**：环境自检、审计链校验、样例账本、规则干跑。
- **费率表**：按事件时间选生效版本（厂商调价不改历史账本）；出厂内嵌表兜底；DeepSeek 高峰价 + Search/Fetch 按次价（2026-11-02 官方定价页核验）。
- **发布基建**：install.sh（纯用户态、sha256 校验）、GitHub Actions release（Linux musl 静态 + Windows zip）。

### 已知限制
- Windows 原生钩子未支持（WSL 内可用）；macOS 在 v0.5。
- PDF 渲染依赖本机 Chrome/Edge，缺失时自动降级为 HTML 交付。
- 高峰/空闲时段化计价未实现（统一按高峰价，略高估，口径已在账单声明）。
