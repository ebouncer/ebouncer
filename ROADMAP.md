# 路线图

## v0.1（当前，2026-11）—— 记账与开票闭环
- [x] 跨工具账本：DSH（真实日志验证）/ Claude Code / Codex（适配器就绪，契约待真实样本回归）
- [x] 客户归因与账单：`ebc tag` / `ebc bill`（table/csv/json/pdf，markup，3 张无水印试用）
- [x] 单币种计价：DeepSeek（CNY）+ Anthropic（USD），混币客户明确拒绝；多币种汇率层随后
- [x] 命令防火墙 dry-run（shell 级）：`ebc init` 注入交互式 shell 钩子，命中规则落审计链（落盘前脱敏）+ 尾部 checkpoint（截断可发现）
- [x] 费率表按事件时间选生效版本（厂商调价，历史账本零迁移）
- [x] 一行安装 + CI/发布流水线（Linux musl 静态 / Windows，install-smoke 端到端）

## v0.2 —— 误报校准与恢复能力
- [ ] 宿主原生 hook 适配：`ebc init --host claude|codex` 写 PreToolUse（dry-run / enforce 两档），并输出托管配置模板（agent 改不掉的那层）
- [ ] 审计外部锚点：checkpoint 定期归档到独立介质/远端只写存储（对抗同 UID 同步改写）
- [ ] 误报率数据驱动的规则库校准（beta 反馈 → 每周规则快照）
- [ ] Shadow Vault：agent 删不掉的影子快照 + `ebc restore`（变更前自动留版本）
- [x] 高峰/空闲时段化计价（2026-11 提前于 v0.1 完成，按事件本地时间取价档）；更多厂商费率表
- [ ] 多币种汇率快照：混币客户账单（USD/CNY 换算，汇率可追溯）
- [ ] `ebc rates check`：定价页变化监测与提醒
- [ ] `ebc rules explain <id>`：命中解释与规则样例

## v0.3 —— 强制与治理
- [ ] 拦截模式（enforce）：从 dry-run 毕业，误报率达标后默认开
- [ ] eBPF/LSM 探针通道：不可绕过的内核级强制
- [ ] 企业受管模式：管理员签名基线、策略只可收紧、集中审计归档
- [ ] 信任基建：cosign 签名 + SLSA provenance + 可复现构建
- [ ] 规则集市（社区 PR + 签名快照 + 高级行业包）

## v0.5+ —— 平台与市场扩展
- [ ] Windows 原生（PowerShell CommandPreExecution 钩子）
- [ ] macOS（zsh + OpenBSM）
- [ ] MCP 执行侧强制（manifest 固定、子进程 spawn 管控）
- [ ] CI 守门（guard-ci → `ebc ci`，SARIF 报告）
- [ ] 凭据守门（secret 清洗 + 短时效最小代理权）

## 明确不做
AI 网关 / prompt 代理（不碰模型流量）、通用多云 FinOps、敏感数据识别模型、iOS/鸿蒙端拦截。
