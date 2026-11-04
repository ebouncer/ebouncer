// SPDX-License-Identifier: AGPL-3.0-only
//! ebc —— eBouncer CLI。设计原则（傻瓜化改造，2026-11-03）：
//! 1. 无参数 = 向导：自动探测宿主 → 自动补账 → 直接给花费结论与下一步；
//! 2. 零记忆：tag/bill 互相给可复制的下一步命令；--days 统一默认 30；
//! 3. demo 纯内存演示，绝不污染真实账本；ingest 事件级幂等（续写会话可增量补账）。
//! 命令名刻意避开 `eb`（与 AWS EB CLI 冲突）。

use chrono::{DateTime, Duration, Utc};
use clap::{Parser, Subcommand};
use ebouncer_adapters::{
    normalize_project, Adapter, claude_code::ClaudeCodeAdapter, codex::CodexAdapter,
    dsh::DshAdapter,
};
use ebouncer_core::{audit, ledger, rates, rules};
mod hook;
use std::collections::{BTreeMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "ebc", version, about = "eBouncer —— OS-level bouncer for AI coding agents")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 内存演示账本效果（不写任何文件）
    Demo,
    /// 汇总近期账本：按工具 × 项目 × 客户 × 模型
    Ledger {
        #[arg(long, default_value = "30")]
        days: i64,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 扫描宿主日志增量入账（无参数 = 自动探测 DSH/Claude Code/Codex）
    Ingest {
        #[arg(long)]
        claude_dir: Option<PathBuf>,
        #[arg(long)]
        codex_dir: Option<PathBuf>,
        #[arg(long)]
        dsh_dir: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 重建账本：旧账本改名保留（永不删除），从宿主日志全量重新入账
    Rebuild {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 给当前（或 --dir）项目打客户标签，并重新归因存量账目
    Tag {
        client: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 出账：无参数 = 列出可开票客户；--client 出该客户账单（table/csv/json/pdf）
    Bill {
        #[arg(long)]
        client: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        markup: f64,
        #[arg(long, default_value = "30")]
        days: i64,
        #[arg(long, value_enum, default_value = "table")]
        format: BillFmt,
        /// 导出到 <data>/exports（永不覆盖，冲突自动加序号）
        #[arg(long)]
        out: bool,
        /// 无水印版（前 3 张免费，之后带水印）
        #[arg(long)]
        trial_clean: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 审计链完整性校验
    AuditVerify {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 规则自检：stdin 每行命令输出 dry-run 决策
    RulesCheck {
        #[arg(long)]
        rules: PathBuf,
    },
    /// 环境自检
    Doctor,
    /// 注入 shell 钩子（自动探测 $SHELL；预览制，--yes 才写 rc）
    Init {
        #[arg(long, value_enum)]
        shell: Option<ShellKind>,
        #[arg(long)]
        yes: bool,
    },
    /// 移除 rc 钩子；--purge 清可再生状态（账本与审计链始终保留）
    Uninstall {
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 由 shell rc 每命令调用：匹配规则→记审计→dry-run 提示（永不拦截、永不非零退出）
    Hook {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum ShellKind {
    Bash,
    Zsh,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum BillFmt {
    Table,
    Csv,
    Json,
    Pdf,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_data_dir(cli: &Option<PathBuf>) -> PathBuf {
    cli.clone().unwrap_or_else(|| home_dir().join(".ebouncer"))
}

/// 探测三个宿主的日志根目录（存在才返回）。
fn detect_hosts() -> Vec<(&'static str, PathBuf)> {
    let mut v = Vec::new();
    let dsh = home_dir().join(".dsh").join("sessions");
    if dsh.exists() {
        v.push(("DSH", dsh));
    }
    let cc = home_dir().join(".claude").join("projects");
    if cc.exists() {
        v.push(("Claude Code", cc));
    }
    let cx = home_dir().join(".codex").join("sessions");
    if cx.exists() {
        v.push(("Codex", cx));
    }
    v
}

fn detect_shell() -> ShellKind {
    match std::env::var("SHELL")
        .ok()
        .as_deref()
        .and_then(|s| s.rsplit(['/', '\\']).next())
    {
        Some("zsh") => ShellKind::Zsh,
        _ => ShellKind::Bash,
    }
}

/// 费率表：<data>/rates 优先，空则回退出厂内嵌。
fn load_rates(data_dir: &Path) -> Vec<rates::VendorRates> {
    let t = rates::load_dir(&data_dir.join("rates"));
    if !t.is_empty() {
        return t;
    }
    rates::parse_yaml(ebouncer_rules::rates::DEEPSEEK)
}

fn collect_logs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_logs(&p, out);
        } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name.ends_with(".jsonl") || name.ends_with(".jsonl.zstd") {
                out.push(p);
            }
        }
    }
}

/// 事件幂等键：(session_id, ts)。续写会话只会新增行，旧行键不变。
fn ev_key(e: &ledger::LedgerEvent) -> (String, String) {
    (e.session_id.clone(), e.ts.clone())
}

/// 把一个宿主目录的事件增量入账；返回新增条数。seen 跨目录共享。
fn ingest_dir(root: &Path, ad: &dyn Adapter, out: &Path, seen: &mut HashSet<(String, String)>) -> usize {
    let mut files = Vec::new();
    collect_logs(root, &mut files);
    let mut n = 0usize;
    for p in files {
        for ev in ad.parse_file(&p) {
            if seen.insert(ev_key(&ev)) {
                if ledger::append(out, &ev).is_ok() {
                    n += 1;
                }
            }
        }
    }
    n
}

fn do_ingest(
    claude_dir: &Option<PathBuf>,
    codex_dir: &Option<PathBuf>,
    dsh_dir: &Option<PathBuf>,
    dir: &Path,
) -> usize {
    let out = dir.join("ledger.jsonl");
    let (existing, _) = ledger::read_all(&out).unwrap_or_else(|_| (Vec::new(), 0));
    let mut seen: HashSet<(String, String)> = existing.iter().map(ev_key).collect();
    let mut n = 0usize;
    let client_map = ebouncer_core::attrib::ClientMap::load(&dir.join("config").join("client_map.yaml"));

    let cc = claude_dir.clone().or_else(|| {
        let d = home_dir().join(".claude").join("projects");
        d.exists().then_some(d)
    });
    if let Some(root) = cc {
        n += ingest_dir(&root, &ClaudeCodeAdapter { session_hint: "cc".to_string() }, &out, &mut seen);
    }
    let cx = codex_dir.clone().or_else(|| {
        let d = home_dir().join(".codex").join("sessions");
        d.exists().then_some(d)
    });
    if let Some(root) = cx {
        n += ingest_dir(&root, &CodexAdapter, &out, &mut seen);
    }
    let dsh = dsh_dir.clone().or_else(|| {
        let d = home_dir().join(".dsh").join("sessions");
        d.exists().then_some(d)
    });
    if let Some(root) = dsh {
        n += ingest_dir(&root, &DshAdapter { client_map }, &out, &mut seen);
    }
    n
}

/// 窗口内一组 (tool, project, client, model) 的数量合计与金额。
#[derive(Default)]
struct LedgerRow {
    tokens_in: u64,
    tokens_out: u64,
    cache_read: u64,
    requests_search: u64,
    requests_fetch: u64,
    cost: f64,
}

/// 按 (tool, project, client, model) 聚合窗口内事件。
/// 金额**逐事件**按各自时间选生效费率版本再求和：同一行可能跨调价窗口，
/// 用单个时间点为整行估价会系统性算错（2026-11-04 修复）。
fn aggregate_window(
    evs: &[ledger::LedgerEvent],
    table: &[rates::VendorRates],
    cutoff: &str,
) -> BTreeMap<(String, String, String, String), LedgerRow> {
    let mut agg: BTreeMap<(String, String, String, String), LedgerRow> = Default::default();
    for e in evs {
        if e.ts.as_str() < cutoff {
            continue;
        }
        let key = (
            e.tool.clone(),
            e.project.clone(),
            e.client.clone().unwrap_or_else(|| "-".to_string()),
            e.model.clone().unwrap_or_else(|| "-".to_string()),
        );
        let row = agg.entry(key).or_default();
        row.tokens_in += e.tokens_in;
        row.tokens_out += e.tokens_out;
        row.cache_read += e.cache_read;
        row.requests_search += e.requests_search;
        row.requests_fetch += e.requests_fetch;
        row.cost += rates::estimate(e, table);
    }
    agg
}

/// 账本聚合 + 表格打印（ledger / 向导共用）。返回是否有数据。
fn print_ledger(dir: &Path, days: i64) -> bool {
    let path = dir.join("ledger.jsonl");
    let (evs, bad) = ledger::read_all(&path).unwrap_or_else(|_| (Vec::new(), 0));
    if evs.is_empty() {
        return false;
    }
    let cutoff = (Utc::now() - Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let table = load_rates(dir);
    let agg = aggregate_window(&evs, &table, &cutoff);
    println!("近 {days} 天花费汇总（工具 × 项目 × 客户 × 模型）");
    println!(
        "{:<13} {:<16} {:<16} {:<18} {:>10} {:>9} {:>9} {:>7} {:>6} {:>8}",
        "tool", "project", "client", "model", "tok_in", "tok_out", "cache_rd", "search*", "fetch*", "≈¥"
    );
    let mut top: Vec<(f64, (String, String, String, String))> = Vec::new();
    for (k, a) in &agg {
        top.push((a.cost, k.clone()));
        println!(
            "{:<13} {:<16} {:<16} {:<18} {:>10} {:>9} {:>9} {:>7} {:>6} {:>8.2}",
            k.0, k.1, k.2, k.3, a.tokens_in, a.tokens_out, a.cache_read, a.requests_search, a.requests_fetch, a.cost
        );
    }
    top.sort_by(|a, b| b.0.total_cmp(&a.0));
    if let Some((c, k)) = top.first() {
        println!("\n花费最大：{} / {}（客户 {}，模型 {}），≈¥{:.2}", k.0, k.1, k.2, k.3, c);
    }
    if bad > 0 {
        eprintln!("（跳过了 {bad} 行无法解析的记录）");
    }
    true
}

/// ebc 无参数 = 向导。
fn wizard() -> i32 {
    let dir = resolve_data_dir(&None);
    println!("eBouncer 向导 —— 一切都在本机，数据只写 {}", dir.display());

    // 1) 宿主探测 + 自动补账
    let hosts = detect_hosts();
    if hosts.is_empty() {
        println!("\n未发现 DSH / Claude Code / Codex 的本地日志目录。");
        println!("装过其中任意一个再回来；或先跑 `ebc demo` 看效果。");
        return 0;
    }
    let names: Vec<&str> = hosts.iter().map(|(n, _)| *n).collect();
    println!("发现宿主：{}", names.join("、"));
    let n = do_ingest(&None, &None, &None, &dir);
    if n > 0 {
        println!("新增入账 {n} 条事件。");
    } else {
        println!("账本已是最新（无新增事件）。");
    }

    // 2) 花费结论
    println!();
    if !print_ledger(&dir, 30) {
        println!("账本暂无近 30 天数据。");
    }

    // 3) 下一步建议
    println!("\n下一步：");
    let rc_hit = ["bash", "zsh"]
        .iter()
        .any(|s| std::fs::read_to_string(hook::rc_path(&home_dir(), s)).map(|t| t.contains(hook::BEGIN)).unwrap_or(false));
    if !rc_hit {
        println!("  ebc init --yes                # 开启命令观察（自动探测 shell，先跑 ebc init 可预览）");
    }
    let has_client = ledger::read_all(&dir.join("ledger.jsonl"))
        .map(|(evs, _)| evs.iter().any(|e| e.client.is_some()))
        .unwrap_or(false);
    if has_client {
        println!("  ebc bill                      # 看能给哪些客户开票");
    } else {
        println!("  cd <项目目录> && ebc tag 客户名 # 给项目挂客户，账单自动归因");
    }
    println!("  ebc doctor                    # 环境体检");
    0
}

fn cmd_tag(client: &str, proj_dir: &Path, data_dir: &Path) -> i32 {
    // 1) 项目里落标签文件（未来的 ingest 自动带上）
    let marker = proj_dir.join(".ebouncer").join("client");
    let target = proj_dir.display().to_string();
    match std::fs::create_dir_all(marker.parent().unwrap()).and_then(|_| std::fs::write(&marker, format!("{client}\n"))) {
        Ok(_) => println!("✔ 已标记项目 → 客户「{client}」（{}）", marker.display()),
        Err(e) => {
            eprintln!("写标签失败：{e}（确认当前在该项目目录内，或用 --dir 指定）");
            return 1;
        }
    }
    // 2) 存量账目重新归因（原子重写）
    let path = data_dir.join("ledger.jsonl");
    let (mut evs, _) = ledger::read_all(&path).unwrap_or_else(|_| (Vec::new(), 0));
    let want = normalize_project(&target);
    let mut touched = 0usize;
    for e in evs.iter_mut() {
        if e.project == want {
            e.client = Some(client.to_string());
            touched += 1;
        }
    }
    if touched > 0 {
        if let Err(e) = ledger::rewrite(&path, &evs) {
            eprintln!("账本重写失败：{e}");
            return 1;
        }
        println!("✔ 已重新归因存量 {touched} 条事件（项目「{want}」）");
        println!("下一步：ebc bill --client \"{client}\" --format pdf --out");
    } else {
        println!("（项目「{want}」暂无存量账目；下次 ebc ingest 的新事件会自动带此客户）");
        println!("下一步：ebc ingest && ebc bill");
    }
    0
}

fn cmd_bill_clients(dir: &Path, days: i64) -> i32 {
    let (evs, bad) = ledger::read_all(&dir.join("ledger.jsonl")).unwrap_or_else(|_| (Vec::new(), 0));
    let cutoff = (Utc::now() - Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let table = load_rates(dir);
    // 与 print_ledger 同源：逐事件按各自时间选费率版本，再按客户累加。
    let mut by_client: BTreeMap<String, f64> = Default::default();
    for e in evs.iter().filter(|e| e.ts >= cutoff) {
        let client = e.client.clone().unwrap_or_else(|| "(未归因)".to_string());
        *by_client.entry(client).or_insert(0.0) += rates::estimate(e, &table);
    }
    if bad > 0 {
        eprintln!("（跳过了 {bad} 行无法解析的记录）");
    }
    if by_client.is_empty() {
        println!("近 {days} 天无账目。先跑 ebc ingest");
        return 0;
    }
    println!("近 {days} 天可开票对象：");
    let mut rows: Vec<(String, f64)> = by_client.iter().map(|(k, v)| (k.clone(), *v)).collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (c, v) in &rows {
        println!("  {:<24} ≈¥{:.2}", c, v);
    }
    if let Some((top, _)) = rows.first() {
        println!("\n下一步：ebc bill --client \"{top}\" --format pdf --out");
    }
    0
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    p
}

fn parse_cutoff(days: i64) -> String {
    (Utc::now() - Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

fn ts_is_valid(s: &str) -> bool {
    DateTime::parse_from_rfc3339(s).is_ok()
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        None => wizard(),
        Some(c) => run(c),
    };
    std::process::exit(code);
}

fn run(cmd: Cmd) -> i32 {
    match cmd {
        Cmd::Demo => {
            // 纯内存演示：一个字节不落盘
            let dir = resolve_data_dir(&None);
            // 固定取样时间（周二 10:00 北京时间 = 高峰档）：demo 不该因为晚上跑就显示半价
            let now = DateTime::parse_from_rfc3339("2026-11-03T02:00:00Z")
                .expect("固定样例时间恒可解析")
                .with_timezone(&Utc);
            let samples: [ledger::LedgerEvent; 3] = [
                demo_ev("claude-code", "acme", Some("ACME 公司"), "claude-sonnet-4-5", 4_000_000, 600_000, 0, 0, now),
                demo_ev("codex", "billing", Some("ACME 计费项目"), "deepseek-chat", 2_000_000, 400_000, 2_000, 0, now),
                demo_ev("dsh", "side-project", None, "deepseek-chat", 900_000, 300_000, 800, 400, now),
            ];
            println!("演示账本（内存中，不写入任何文件）：\n");
            let table = load_rates(&dir);
            println!(
                "{:<13} {:<16} {:<16} {:<18} {:>10} {:>9} {:>7} {:>6} {:>8}",
                "tool", "project", "client", "model", "tok_in", "tok_out", "search*", "fetch*", "≈¥"
            );
            for e in &samples {
                let est = rates::estimate(e, &table);
                println!(
                    "{:<13} {:<16} {:<16} {:<18} {:>10} {:>9} {:>7} {:>6} {:>8.2}",
                    e.tool, e.project,
                    e.client.clone().unwrap_or_else(|| "-".into()),
                    e.model.clone().unwrap_or_else(|| "-".into()),
                    e.tokens_in, e.tokens_out, e.requests_search, e.requests_fetch, est
                );
            }
            println!("\n（claude 行 ¥0 属预期：未配置该模型费率时只记量不算价，绝不编价）");
            println!("真实数据：ebc ingest → ebc ledger");
            0
        }

        Cmd::Ledger { days, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            if !print_ledger(&dir, days) {
                eprintln!("账本为空（{}）。", dir.join("ledger.jsonl").display());
                let hosts = detect_hosts();
                if hosts.is_empty() {
                    eprintln!("未发现宿主日志——先装/用 DSH、Claude Code 或 Codex；或 ebc demo 看效果。");
                } else {
                    eprintln!("发现宿主日志：{}。跑 `ebc ingest` 记第一笔账。", hosts.iter().map(|(n, _)| *n).collect::<Vec<_>>().join("、"));
                }
                return 1;
            }
            0
        }

        Cmd::Ingest { claude_dir, codex_dir, dsh_dir, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            let n = do_ingest(&claude_dir, &codex_dir, &dsh_dir, &dir);
            println!("已入账 {n} 条新事件 → {}", dir.join("ledger.jsonl").display());
            println!("接着看：ebc ledger --days 30");
            0
        }

        Cmd::Rebuild { data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            let path = dir.join("ledger.jsonl");
            if path.exists() {
                let bak = dir.join(format!(
                    "ledger.jsonl.bak-{}",
                    Utc::now().format("%Y%m%dT%H%M%SZ")
                ));
                match std::fs::rename(&path, &bak) {
                    Ok(_) => println!("旧账本已保留：{}", bak.display()),
                    Err(e) => { eprintln!("改名失败：{e}"); return 1; }
                }
            }
            let n = do_ingest(&None, &None, &None, &dir);
            println!("重建完成：{n} 条事件从宿主日志重新入账");
            0
        }

        Cmd::Tag { client, dir, data_dir } => {
            let proj = dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let data = resolve_data_dir(&data_dir);
            cmd_tag(&client, &proj, &data)
        }

        Cmd::Bill { client, markup, days, format, out, trial_clean, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            let Some(client) = client else {
                return cmd_bill_clients(&dir, days);
            };
            let path = dir.join("ledger.jsonl");
            let (evs, _) = ledger::read_all(&path).unwrap_or_else(|_| (Vec::new(), 0));
            if evs.is_empty() {
                eprintln!("账本为空，先跑 ebc ingest");
                return 1;
            }
            let cutoff = parse_cutoff(days);
            let evs: Vec<_> = evs.into_iter().filter(|e| ts_is_valid(&e.ts) && e.ts >= cutoff).collect();
            let table = load_rates(&dir);

            // 次数级试用：前 3 张无水印
            let trial_file = dir.join("trial.json");
            let mut used: u32 = std::fs::read_to_string(&trial_file)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("clean_invoices").and_then(|n| n.as_u64()))
                .map(|n| n as u32)
                .unwrap_or(0);
            let mut watermark = true;
            if trial_clean {
                if used < 3 {
                    watermark = false;
                    used += 1;
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(&trial_file, format!("{{\"clean_invoices\": {used}}}"));
                    eprintln!("无水印试用额度：{used}/3 已用");
                } else {
                    eprintln!("无水印额度已用完（3/3），本次带水印。");
                }
            }

            let basis = ledger::file_sha256(&path).ok();
            let inv = ebouncer_core::bill::build(&client, &evs, &table, markup, watermark, basis);
            if inv.items.is_empty() {
                let known: Vec<String> = {
                    let mut s: Vec<String> = evs.iter().filter_map(|e| e.client.clone()).collect::<Vec<_>>();
                    s.sort();
                    s.dedup();
                    s
                };
                eprintln!("客户「{client}」近 {days} 天无账目。");
                if known.is_empty() {
                    eprintln!("账本里还没有任何客户——在项目目录里跑：ebc tag 客户名");
                } else {
                    eprintln!("现有客户：{}。命令可整行复制：", known.join("、"));
                    for k in &known {
                        println!("  ebc bill --client \"{k}\"");
                    }
                }
                return 1;
            }
            match format {
                BillFmt::Json => println!("{}", inv.to_json()),
                BillFmt::Csv => println!("{}", inv.to_csv()),
                BillFmt::Table | BillFmt::Pdf => {
                    println!("客户 {} · 周期 {} ~ {} · 加成 {:.0}% · 水印 {}",
                        inv.client, inv.period.from, inv.period.to, inv.markup * 100.0,
                        if inv.watermark { "是" } else { "否（试用）" });
                    println!("{:<13} {:<15} {:>12} {:>12} {:>12}", "tool", "category", "qty", "unit", "amount");
                    for i in &inv.items {
                        println!("{:<13} {:<15} {:>12} {:>12.4} {:>12.4}", i.tool, i.category, i.qty, i.unit_price_per_k_or_mtok, i.amount);
                    }
                    let basis = inv.data_basis.as_deref().unwrap_or("-");
                    println!(
                        "合计（含加成）：¥{:.2} · 估算未含税 · 账本指纹 {} · 费率表指纹 {}",
                        inv.total,
                        &basis[..basis.len().min(12)],
                        &inv.rates_fingerprint[..inv.rates_fingerprint.len().min(12)]
                    );
                }
            }
            if out {
                let exp = dir.join("exports");
                let _ = std::fs::create_dir_all(&exp);
                let safe_client: String = client
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                    .collect::<String>()
                    .trim_matches('-')
                    .to_string();
                let safe_client = if safe_client.is_empty() { "client".to_string() } else { safe_client };
                let stem = format!("invoice-{safe_client}-{}", Utc::now().format("%Y%m%d"));
                if matches!(format, BillFmt::Pdf) {
                    let html_p = unique_path(&exp, &stem, "html");
                    if std::fs::write(&html_p, ebouncer_core::render::invoice_html(&inv)).is_err() {
                        eprintln!("HTML 渲染失败");
                        return 1;
                    }
                    let pdf_p = unique_path(&exp, &stem, "pdf");
                    match ebouncer_core::render::html_to_pdf(&html_p, &pdf_p) {
                        Ok(_) => println!("已导出：{}\n      （源 HTML：{}）", pdf_p.display(), html_p.display()),
                        Err(e) => eprintln!("PDF 转换降级：{e}\n已导出：{}", html_p.display()),
                    }
                    return 0;
                }
                let (ext, body): (&str, String) = match format {
                    BillFmt::Json => ("json", inv.to_json()),
                    _ => ("csv", inv.to_csv()),
                };
                let p = unique_path(&exp, &stem, ext);
                match std::fs::write(&p, body) {
                    Ok(_) => println!("已导出：{}", p.display()),
                    Err(e) => { eprintln!("导出失败：{e}"); return 1; }
                }
            }
            0
        }

        Cmd::AuditVerify { data_dir } => {
            let p = resolve_data_dir(&data_dir).join("audit.jsonl");
            match audit::verify(&p) {
                Ok(r) if r.is_ok() => {
                    if r.checkpoint_checked {
                        println!("审计链完好（{} 条，checkpoint 已校验）", r.lines);
                    } else {
                        println!(
                            "审计链完好（{} 条；无 checkpoint，无法检测尾部截断——下次命中命令后自动生成）",
                            r.lines
                        );
                    }
                    0
                }
                Ok(r) => {
                    if let Some((expected, found)) = r.truncated {
                        eprintln!(
                            "审计链被截断：checkpoint 记录 {expected} 条，文件只有 {found} 条——尾部记录被删"
                        );
                    } else {
                        eprintln!("第 {} 行断链——审计可能被篡改", r.broken_at);
                    }
                    2
                }
                Err(e) => { eprintln!("校验失败：{e}"); 1 }
            }
        }

        Cmd::RulesCheck { rules: rules_path } => {
            let set = match rules::RuleSet::load(&rules_path) {
                Ok(s) => s,
                Err(e) => { eprintln!("规则加载失败：{e}"); return 1; }
            };
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let cmd = line.trim();
                if cmd.is_empty() { continue; }
                let (decision, id) = set.evaluate(cmd, true);
                println!("{decision:<9} {:<16} {cmd}", id.unwrap_or_else(|| "-".to_string()));
            }
            0
        }

        Cmd::Init { shell, yes } => {
            let shell = shell.unwrap_or_else(detect_shell);
            hook::init(&home_dir(), match shell { ShellKind::Bash => "bash", ShellKind::Zsh => "zsh" }, yes)
        }

        Cmd::Uninstall { purge, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            hook::uninstall(&home_dir(), &dir, purge)
        }

        Cmd::Hook { data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            hook::hook(&dir)
        }

        Cmd::Doctor => {
            println!("ebc doctor —— 环境自检（v0.3 补签名/内核 BTF 检查）");
            let platform = match std::env::consts::OS {
                "windows" if std::env::var("MSYSTEM").is_ok() => "windows (MSYS2/类Unix shell)".to_string(),
                o => o.to_string(),
            };
            println!("  平台: {platform}");
            let dir = resolve_data_dir(&None);
            println!("  数据目录: {} {}", dir.display(), if dir.exists() { "已存在" } else { "未初始化" });
            for (name, p) in [
                ("DSH 日志", home_dir().join(".dsh").join("sessions")),
                ("Claude Code 日志", home_dir().join(".claude").join("projects")),
                ("Codex 日志", home_dir().join(".codex").join("sessions")),
            ] {
                println!("  {name}: {}", if p.exists() { "发现" } else { "未发现/未安装" });
            }
            for shell in ["bash", "zsh"] {
                let rc = hook::rc_path(&home_dir(), shell);
                let injected = std::fs::read_to_string(&rc).map(|s| s.contains(hook::BEGIN)).unwrap_or(false);
                println!("  {shell} 钩子: {}", if injected { "已注入" } else { "未注入（ebc init --yes）" });
            }
            let audit_p = dir.join("audit.jsonl");
            if audit_p.exists() {
                match audit::verify(&audit_p) {
                    Ok(r) if r.is_ok() => println!(
                        "  审计链: 完好{}",
                        if r.checkpoint_checked { "（checkpoint 已校验）" } else { "（无 checkpoint：检测不了尾部截断）" }
                    ),
                    Ok(r) => match r.truncated {
                        Some((expected, found)) => {
                            println!("  审计链: ⛔ 尾部截断（checkpoint {expected} 条 / 文件 {found} 条）")
                        }
                        None => println!("  审计链: ⛔ 第 {} 行断链", r.broken_at),
                    },
                    Err(_) => println!("  审计链: 读取失败"),
                }
            } else {
                println!("  审计链: 尚无记录（钩子命中命令后生成）");
            }
            0
        }
    }
}

fn demo_ev(
    tool: &str, project: &str, client: Option<&str>, model: &str,
    tin: u64, tout: u64, sr: u64, sf: u64, now: DateTime<Utc>,
) -> ledger::LedgerEvent {
    ledger::LedgerEvent {
        ts: now.to_rfc3339(),
        session_id: format!("demo-{tool}"),
        tool: tool.to_string(),
        project: project.to_string(),
        client: client.map(str::to_string),
        model: Some(model.to_string()),
        tokens_in: tin,
        tokens_out: tout,
        cache_read: 0,
        cache_write: 0,
        cost_native: 0.0,
        currency: "CNY".to_string(),
        requests_search: sr,
        requests_fetch: sf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: &str, project: &str, tokens_in: u64) -> ledger::LedgerEvent {
        ledger::LedgerEvent {
            ts: ts.to_string(),
            session_id: format!("s-{project}"),
            tool: "dsh".to_string(),
            project: project.to_string(),
            client: None,
            model: Some("deepseek-chat".to_string()),
            tokens_in,
            tokens_out: 0,
            cache_read: 0,
            cache_write: 0,
            cost_native: 0.0,
            currency: "CNY".to_string(),
            requests_search: 0,
            requests_fetch: 0,
        }
    }

    fn rate(effective_from: &str, input_per_mtok: f64) -> rates::VendorRates {
        rates::VendorRates {
            vendor: "deepseek".to_string(),
            currency: "CNY".to_string(),
            effective_from: effective_from.to_string(),
            time_tiers: None,
            models: vec![rates::ModelRate {
                name: "deepseek-chat".to_string(),
                input_per_mtok,
                output_per_mtok: 0.0,
                cache_read_per_mtok: 0.0,
                cache_write_per_mtok: 0.0,
            }],
            tool_requests: None,
        }
    }

    /// 回归（2026-11-04）：整行用空时间戳估价，会让跨调价窗口的行永远落到最早费率版本。
    /// 两个同组事件分别落在旧价期(1.0)与新价期(2.0)，逐事件计价应为 3.0；
    /// 若整行只取一个版本，会得到 2.0（最早版本）或 4.0（最新版本）。
    #[test]
    fn window_cost_prices_each_event_by_its_own_rate_version() {
        let table = vec![rate("2026-09-01", 1.0), rate("2026-11-01", 2.0)];
        let evs = vec![
            ev("2026-10-15T00:00:00Z", "billing", 1_000_000),
            ev("2026-11-03T00:00:00Z", "billing", 1_000_000),
        ];
        let agg = aggregate_window(&evs, &table, "2026-10-01T00:00:00Z");
        assert_eq!(agg.len(), 1, "同一 (tool, project, client, model) 应聚合成一行");
        let row = agg.values().next().unwrap();
        assert!((row.cost - 3.0).abs() < 1e-9, "cost={}", row.cost);
        assert_eq!(row.tokens_in, 2_000_000);
    }

    /// 同一窗口内不同项目各自取价，且窗口外事件不参与聚合。
    #[test]
    fn window_aggregation_filters_cutoff_and_prices_per_row() {
        let table = vec![rate("2026-09-01", 1.0), rate("2026-11-01", 2.0)];
        let evs = vec![
            ev("2026-09-15T00:00:00Z", "old", 5_000_000), // 窗口外，必须被过滤
            ev("2026-10-15T00:00:00Z", "new", 1_000_000), // 旧价 → 1.0
            ev("2026-11-03T00:00:00Z", "later", 1_000_000), // 新价 → 2.0
        ];
        let agg = aggregate_window(&evs, &table, "2026-10-01T00:00:00Z");
        assert_eq!(agg.len(), 2);
        let cost_of = |project: &str| {
            agg.iter()
                .find(|(k, _)| k.1 == project)
                .map(|(_, r)| r.cost)
                .unwrap()
        };
        assert!((cost_of("new") - 1.0).abs() < 1e-9);
        assert!((cost_of("later") - 2.0).abs() < 1e-9);
    }
}
