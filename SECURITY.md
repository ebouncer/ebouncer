# 安全策略（v0.1 → v0.3 信任基建路线图）

## 当前版本承诺
- 纯本地运行，无网络请求（费率表更新除外，后续做成签名可离线验证）。
- 数据目录 0700；二进制不带 setuid、不开 cap；probe 通道（内核 eBPF）尚未发布。
- 审计链哈希自证：`ebc audit-verify` 可检出任何篡改断链。

## 计划中（v0.3，对应 PRD §10.2 信任基建）
- cosign 签名 + SLSA L3 provenance + 每 release 的 SBOM（注意：SBOM 建议提前进 v0.1——欧盟新产品责任指令 2026-12-09 起适用，自证证据链越早越好）。
- `ebc doctor --sec` 一键验证本机安装的来源与完整性。

## 已知攻击面（诚实清单）
1. preexec 钩子运行于用户自身 shell——能控制该 shell 者本就能伪造账本；v0.3 的 eBPF 通道才改变此威胁模型。
2. ledger 解析依赖上游日志格式，改版会导致漏记（fail-open 并告警），不会静默错记。

## 报告漏洞
GitHub Security Advisory（发布 org 后启用）。请勿公开 issue。
