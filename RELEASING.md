# 发布流程

## 渠道与版本

| 渠道 | 触发 | install.sh |
|---|---|---|
| 稳定版 `vX.Y.Z` | 手动打 tag | `EBC_CHANNEL=stable` |
| 预发布 `vX.Y.Z-rc.N` | 手动打 tag | 默认 `auto`（取最新一条，含 rc）；`EBC_CHANNEL=rc` 同义 |
| 固定版本 | — | `EBC_VERSION=vX.Y.Z` |

> `auto` 是默认值：v0.1.0 正式发布前没有稳定版，必须能装到 rc，否则会拿到旧的 rc.2。
> 打正式版 tag 后，最新一条就是稳定版，`auto` 自然切过去。

版本语义：`0.x` 期间允许破坏性变更；账本/发票 schema 有版本号，读旧数据必须兼容。

## 发布纪律（2026-11 教训）

1. **tag = 对外交付，不是 CI**。验证构建用 `ci` 的 `release-dry-run`（workflow_dispatch），不要靠连打 rc 试管道。
2. **一个候选窗口只打一个 rc**。失败就修，但修复攒到下一个 rc；不要在 rc 之间夹新功能。
3. **候选冻结**后只修 P0（安装失败/数据损坏/金额错误）；功能排到下一个 minor。
4. rc 必须标 prerelease；只有正式版才进入 `EBC_CHANNEL=stable`。

## 发布门槛（v0.1.0 基础版）

1. `cargo test --workspace` 全绿；本地 `ebc demo` / `ebc ledger` / `ebc bill` / `ebc audit-verify` 冒烟通过。
2. 安装闭环：release workflow 的 `install-smoke`（Linux）通过；Git-Bash/MSYS2 真机装一次。
3. 计价：DeepSeek（CNY）与 Anthropic（USD）各出一张单，金额与账本复算一致；混币客户明确报错。
4. 审计：删尾部/删文件能被 `audit-verify` 发现；`uninstall` 保留账本与审计。
5. 文档与实现一致：README 支持矩阵、BETA 命令、CHANGELOG、ROADMAP。
6. 发布物：Linux musl + Windows zip + `checksums.txt`；rc 必须标记 prerelease。

## 发版步骤

```bash
# 1) 版本号与 CHANGELOG
#    Cargo.toml workspace.version 与 CHANGELOG 的 [X.Y.Z] 对齐
# 2) 本地门禁（CI 同款硬门）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# 2.5) 行为验收：G2–G6 自动化部分（demo/空账本/审计截断/混币/证据包/安装脚本语法）
bash scripts/release-check.sh
# 3) 可选：CI 手动跑 release-dry-run（构建 + 二进制冒烟，不建 release）
#    gh workflow run ci.yml
# 4) 打 tag（rc 带 -rc.N）
git tag -a vX.Y.Z-rc.1 -m "vX.Y.Z-rc.1"
git push origin vX.Y.Z-rc.1
# 5) 等 release workflow：双平台构建 → checksums → GitHub Release → install-smoke
# 6) soak 5–7 天（beta 用户），只修 P0/P1
# 7) 稳定版
git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
```

## 回滚

- 装旧版：`EBC_VERSION=vX.Y.Z-1 sh install.sh`。
- 发布有严重问题：把该 release 标为 prerelease（或删资产），在 release notes 说明；不要改已发布 tag 的内容。
- 数据兼容：账本只追加；`ebc rebuild` 会保留旧账本备份，不删除。

## 发布后检查

- `curl | sh` 装到的版本 = 刚发布的 tag（stable 渠道不装 rc）。
- `ebc doctor` 无红项；`ebc audit-verify` 在真实数据上通过。
- 下载量 / issue 里是否有安装失败报告。
