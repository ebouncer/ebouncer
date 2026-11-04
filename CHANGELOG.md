# 更新日志

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增
- **订阅月费账单**：`subscription` 模式的 provider 默认月费直通；`--allocate` 按该 provider 的 token 占比分摊（含其他客户）。订阅快照写入 `invoice.subscriptions`，证据包可复算。
- **MiniMax 按量费率**：M3.1-Flash / M3 / `minimax` 别名（CNY）；只有 `ebc plan set minimax --mode payg` 才启用，订阅/unknown 不套 token 单价。
- **计费模式识别（`ebc plan`）**：`show` / `detect` / `set` / `forget`；自动探测 MiniMax（订阅 vs 按量）、DeepSeek（按量+余额）、OpenRouter、Anthropic、OpenAI、自定义端点。Key 只在内存使用，结果缓存、不落盘 Key；识别失败为 `unknown`，只记量、金额 0，不猜价。
- **安装体验**：Windows 原生 `install.ps1`（用户态、sha256 校验、自动写用户 PATH）；Linux `install.sh` 自动把 `~/.local/bin` 写进 shell rc；新增 [INSTALL.md](INSTALL.md)（Linux/Windows 安装、开票、升级/卸载、排错）。
- **开票简化**：`ebc bill --client X --pdf`（等价 `--format pdf --out`），`--pdf --bundle` 一次出 PDF + 证据包。
- **高峰/空闲时段化计价**：费率表新增 `time_tiers`（高峰窗口 + 固定 UTC 偏移 + 周末/节假日规则 + 空闲倍数），同一版本内的事件按本地时间落到高峰或空闲价档，账单按价档拆行并在 `price_source` 标注。DeepSeek 出厂表按官方定价页（2026-11-04 核实）配置：北京时间周一至周五 09:00-12:00、14:00-18:00 高峰，空闲半价；节假日需在 `holiday_dates` 显式列出（无法本地推导）。
- 出厂费率表补充 **deepseek-v4-pro**（官方页 2026-11-04 核实：9.0 / 27.0 / 缓存命中 0.3 高峰价）与旧名 `deepseek-v4-flash` 映射。
- **账单价格快照（可复现）**：`ebc bill` 把本单实际命中的费率条目原文写进 `rates_snapshot`，并对快照取 sha256 写入 `rates_fingerprint`；表格/CSV/PDF 页脚都带指纹，CSV/JSON 可回查条目。费率表日后增删版本也不影响已开账单的金额与依据；同一账本 + 同一费率表重开账单指纹一致（新增回归测试）。
- **缓存写入（cache_write）计价**：`ModelRate` 新增 `cache_write_per_mtok`（缺省 0）。此前 cache_write 只记量不计价；配置单价后按 token 计费并随高峰/空闲价档折算，账单/CSV/JSON 单列 `cache_write` 科目（invoice schema 枚举已补）。DeepSeek 出厂表保持 0——官方不对写缓存单独收费，写入已含在"缓存未命中"输入价内，填价会重复计费。
- **Anthropic（USD）出厂费率**：新增 `crates/ebouncer-rules/rates/anthropic.yaml`，覆盖 Fable/Opus/Sonnet/Haiku 当前与上一代共 10 个模型（含 `cache_read` / `cache_write` 单价），模型 ID 支持日期后缀与最长前缀匹配。官方定价页 2026-11-04 核对（并与 nOps 2026-10-19 rate card 交叉验证）；缓存写入按 5 分钟 TTL 价。
- **单币种账单**：`Invoice.currency` 改为跟随命中费率条目的币种（CNY / USD），`ebc ledger` / `ebc bill` 按币种显示 ¥/$；同一客户跨币种时返回 `BillError::MixedCurrency` 明确拒绝，不做隐式汇率换算。
- **可携带证据包（.ebc.zip）**：`ebc bundle --client X` 打包 manifest + SHA256SUMS + invoice + 账本切片 + 费率快照（可选 `--with-audit` 带审计链）；`ebc verify-bundle` 离线校验哈希、用包内账本 + 费率复算账单、校验费率指纹与审计链。ZIP 为 store-only 自实现（不新增依赖）；干净 HOME 下可验证，不读 `~/.ebouncer`。新增 4 条回归测试。

### 变更
- **正式版默认无水印**：账单/证据包默认干净 PDF；`--trial-clean` 保留为兼容参数但无作用；`--watermark` 仅用于演示/截图；不再读写 `trial.json`。
- **成本账单与报价单分离**：`--markup` 默认 0；成本账单标题为「AI 用量账单」，不出现「加成/服务费」字样；只有显式 `--markup > 0` 时才生成「AI 用量报价单」，费用称「服务费 X%」。控制台/CSV/HTML 同步。
- **账单可读性**：PDF/HTML 与控制台表格新增「计价单位 / 价档」列（1M tokens / 1K 次；高峰 / 空闲 / 按次 / 统一），数量加千分位，金额带币种符号；周期只显示日期、生成时间去掉纳秒；新增账单编号 `INV-YYYYMMDD-<账本指纹前 8 位>`。
- **定位纠正（外部评审反馈）**：README/BETA/SECURITY 不再声称"宿主做不到命令级拦截/留痕"——Claude Code / Codex / Cursor 均已自带 hooks，且支持托管配置锁定。现在明确：v0.1 的 shell 钩子只覆盖**交互式终端里人敲的命令**，agent 的 `bash -c` 不经过；文本规则可被 `python -c`/变量拼接/`curl|sh` 绕过；同一 UID 下审计只能"可发现"、不能"不可篡改"。差异化改为跨工具账本/开票/证据可携带，宿主原生 hook 适配排进 v0.2。
- `ebc --help` 定位改为"跨工具 AI 花费账本与审计证据层"；Codex 事件币种改为 USD（原为 CNY，与其真实计费一致）。
- **发布渠道**：rc tag 自动标记 prerelease；`install.sh` 默认 `auto`（取最新 release，含 rc），`EBC_CHANNEL=stable` 只取稳定版；install-smoke 修复"空账本 `ebc ledger` 退出 1"的误判（rc.3–rc.5 卡在这里，rc.6 通过）。

### 修复
- **Claude Code 重复入账**：同一条 API 响应会写成多行共享 `message.id` 的 assistant 行，适配器逐行入账导致 token 虚高（beta 实测 1.95×）。现在按 `(sessionId, message.id)` 去重，token 字段取最大值；115 行/46 个 message.id 的样本回落到与 cost-state 真值 −0.65%。
- **Anthropic Web Search 按次漏计**：解析 `usage.server_tool_use.web_search_requests / web_fetch_requests`；Web Search $10/1K searches，Web Fetch 除 token 外无额外费用。
- **权限**：数据目录在 Unix 下强制 0700、账本/审计文件 0600；Windows 明确由用户 ACL 控制，不再承诺 POSIX 0700。
- **`ebc ... | head` panic**：Broken pipe 不再以 exit 101 崩溃，按正常结束 exit 0；其他 panic 照常打印。
- **全 0 账单提示**：所有条目金额为 0 时明确提示“没有匹配到按量费率/可能是包月订阅”，避免被当成免费；水印状态不再显示“否（试用）”。
- **按次计费生效日前被误收费**：DeepSeek Search/Fetch 自 2026-11-01 起计费，但费率回退把 2026-10 的事件也按 11 月的按次价计入（真实账本多收 ¥0.19）。`ToolRequestRate` 新增 `effective_from`，早于该日期只记量、不计费；账单保留 0 元行并注明原因。
- **审计链尾部截断检测**：`audit-verify` 此前只校验链内 `prev_hash`，删掉最后 N 条仍报"完好"（2026-11-04 实测）。现在每次 append 原子更新 `audit.head` checkpoint（seq + hash），校验时对比：删尾部、删整文件、替换末条都会报"被截断"（退出码 2）；checkpoint 缺失时显式降级提示"无法检测截断"，不假装校验过。新增 4 条回归测试。
- **跨调价窗口的金额算错**：`ebc ledger` / 向导 / `ebc bill` 客户列表此前用空时间戳为整行估价，导致聚合行一律落到最早费率版本；账单又按窗口首事件时间锁价，同一客户跨调价时两个口径还会互相矛盾。现在三方统一为**逐事件**选择生效费率版本再累加，账单按费率条目拆行并各自标注 `price_source`（厂商 + 生效日）。新增 3 条回归测试（聚合计价、账单拆行、账单合计）。
- **版本先后按字符串比较**：`2026-11-01T09:00:00+08:00` 这类带偏移的版本与事件时间混排时字典序≠时间先后。现在优先解析为时间点比较，解析失败才回退字典序；纯日期（`YYYY-MM-DD`）按 UTC 00:00 处理。新增偏移场景回归测试。
- **账本首行带 UTF-8 BOM 时被静默丢弃**：`ledger::read_all` 现在剥离 BOM，账单不再因此少算一笔；新增回归测试。
- **纯 Search/Fetch 事件在账单里被记 0 元**：无模型事件（`model=None`）此前在 `ebc bill` 走"无价模型"分支、金额为 0，而 `ebc ledger` 按按次价计入——同一账本两处金额不一致（真实账本实测差 ¥0.19）。现在账单按事件时间选带按次价的费率条目计费，`price_source` 标注"按次价（无时段折扣）"，与账本口径一致；新增 2 条回归测试。
- **`ebc bill` 列表里的 `(未归因)` 是死入口**：它能被列成可开票对象，但 `--client "(未归因)"` 找不到事件。现在未归因只显示条数与 `ebc tag` 提示，不再出现在可开票列表。
- 账单 `price_source` 不再把事件时间当成"费率表版本"展示。

## [0.1.0] - 目标 2026-11-20（首个公开版本，纯免费开源）

### 新增
- **ebc ledger**：跨工具 AI 花费账本。DSH 已用真实日志验证（多帧 zstd、会话去重）；Claude Code / Codex 适配器就绪，契约待真实样本回归（含 Codex token_count 重发的重复计费规避）。
- **ebc ingest**：递归扫描宿主日志目录入账，K2 优先级客户归因（显式 > 项目 `.ebouncer/client` > git remote 映射）。
- **ebc bill**：按客户出账（table/csv/json/pdf）；markup 仅用于报价模式、估算口径与账本指纹随单；默认无水印；导出永不覆盖。
- **ebc init / hook / uninstall**：shell 级命令防火墙（dry-run，仅覆盖交互式终端）。zsh preexec / bash PROMPT_COMMAND 注入（预览确认制、BEGIN/END 幂等块）；33 条危险命令出厂基线；命中命令落盘前脱敏（D3）写入哈希审计链 + `audit.head` 尾部锚点（可发现截断）；任何内部失败静默放行（fail-open）；卸载保留证据链。
- **ebc doctor / audit-verify / demo / rules-check**：环境自检、审计链校验、样例账本、规则干跑。
- **费率表**：按事件时间选生效版本（厂商调价不改历史账本），版本内再按事件本地时间落到高峰/空闲价档（2026-11-04 官方定价页复核）；出厂内嵌 DeepSeek（CNY）+ Anthropic（USD）两套表，单币种账单；DeepSeek 时段价 + Search/Fetch 按次价（2026-11-02 官方定价页核验）。
- **发布基建**：install.sh（纯用户态、sha256 校验）、GitHub Actions release（Linux musl 静态 + Windows zip）。

### 已知限制
- 命令钩子只覆盖**交互式终端**：agent 用 `bash -c` 执行的命令不经过；宿主原生 hook（Claude Code PreToolUse / Codex hooks）适配在 v0.2。
- 审计链同 UID 可伪造：checkpoint 与审计文件同权限，同时改写并重算链可绕过；只能发现"只删尾部/只改正文"，外部锚点在 v0.2。
- 规则是文本匹配，`python -c`、变量拼接、`curl | sh` 等可绕过；不是沙箱。
- 账单为单币种（CNY 或 USD）；同一客户跨币种时明确报错，多币种汇率层未实现。
- Anthropic 缓存写入按 5 分钟 TTL 计价；日志不区分 5m/1h，若实际用 1h TTL 会略低估。
- Windows 原生钩子未支持（WSL 内可用）；macOS 在 v0.5。
- PDF 渲染依赖本机 Chrome/Edge，缺失时自动降级为 HTML 交付。
- 中国法定节假日需在费率表 `holiday_dates` 手工列出（无法本地推导）；未列出时假日按高峰价，略微高估。
