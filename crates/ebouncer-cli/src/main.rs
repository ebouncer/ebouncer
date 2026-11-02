// SPDX-License-Identifier: AGPL-3.0-only
//! ebc —— eBouncer CLI（W1 最小可用面：demo / ledger / ingest / audit-verify / rules-check / doctor）。
//! 命令名刻意避开 `eb`（与 AWS EB CLI 冲突）。

use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use ebouncer_adapters::{
    Adapter, claude_code::ClaudeCodeAdapter, codex::CodexAdapter, dsh::DshAdapter,
};
use ebouncer_core::{audit, ledger, rates, rules};
mod hook;
use std::io::BufRead;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "ebc", version, about = "eBouncer —— OS-level bouncer for AI coding agents")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 用样例数据生成演示账本，跑通 ledger 全流程
    Demo {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 汇总近期账本：按工具 × 项目 × 客户
    Ledger {
        #[arg(long, default_value = "7")]
        days: i64,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 扫描宿主日志目录 → 写入账本
    Ingest {
        #[arg(long)]
        claude_dir: Option<PathBuf>,
        #[arg(long)]
        codex_dir: Option<PathBuf>,
        /// DSH 会话根目录（默认 ~/.dsh/sessions，递归找 session*.jsonl[.zstd]）
        #[arg(long)]
        dsh_dir: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 审计链完整性校验
    AuditVerify {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 按客户生成账单（CSV/JSON；invoice.pdf 进 W5）。默认 dry-run 打印，--out 落盘且永不覆盖
    Bill {
        #[arg(long)]
        client: String,
        /// 加成比例（0.2 = +20%），报进客户价用
        #[arg(long, default_value_t = 0.0)]
        markup: f64,
        #[arg(long, default_value = "30")]
        days: i64,
        #[arg(long, value_enum, default_value = "table")]
        format: BillFmt,
        /// 导出到 <data_dir>/exports（文件名带客户+日期，冲突自动加序号）
        #[arg(long)]
        out: bool,
        /// 本次为无水印版（前 3 次免费，之后需订阅——§6.2 次数级试用）
        #[arg(long)]
        trial_clean: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 规则自检：对 stdin 每行命令输出 dry-run 决策
    RulesCheck {
        #[arg(long)]
        rules: PathBuf,
    },
    /// 环境自检（平台 / 数据目录 / 宿主日志来源）
    Doctor,
    /// 注入 shell 钩子（默认仅预览，--yes 才写 rc 文件）
    Init {
        #[arg(long, value_enum, default_value = "bash")]
        shell: ShellKind,
        #[arg(long)]
        yes: bool,
    },
    /// 移除 rc 中的 eBouncer 块；--purge 清可再生状态（审计链与账本始终保留）
    Uninstall {
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 由 shell rc 块每命令调用一次：匹配规则→记审计→dry-run 提示（永不拦截、永不 exit 非 0）
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

/// 永不覆盖：同 stem 冲突自动加 -N 后缀。
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    p
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

/// 费率表加载：用户目录 <data>/rates 优先；为空则回退到编译进二进制的出厂表，
/// 保证新用户装完即出真实金额，不必先手动放 yaml（旧 bug：全 ¥0）。
fn load_rates(data_dir: &Path) -> Vec<ebouncer_core::rates::VendorRates> {
    let t = rates::load_dir(&data_dir.join("rates"));
    if !t.is_empty() {
        return t;
    }
    rates::parse_yaml(ebouncer_rules::rates::DEEPSEEK)
}

/// 递归收集目录下的日志文件（兼容 DSH 的嵌套 session 目录与 .jsonl.zstd）。
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

fn ingest_dir(root: &Path, ad: &dyn Adapter, out: &Path) -> usize {
    let mut files = Vec::new();
    collect_logs(root, &mut files);
    // 幂等 + 防双份：DSH 迁移后同一会话在 <uuid>/ 与 session-<uuid>/ 各存一份，
    // 且账本已有历史事件——按 session_id 去重后才入账。
    let (existing, _) = ledger::read_all(out).unwrap_or_else(|_| (Vec::new(), 0));
    let mut seen: std::collections::HashSet<String> =
        existing.into_iter().map(|e| e.session_id).collect();
    let mut n = 0usize;
    let mut dup = 0usize;
    for p in files {
        let evs = ad.parse_file(&p);
        let Some(sid) = evs.first().map(|e| e.session_id.clone()) else {
            continue;
        };
        if !seen.insert(sid) {
            dup += evs.len();
            continue;
        }
        for ev in evs {
            if ledger::append(out, &ev).is_ok() {
                n += 1;
            }
        }
    }
    if dup > 0 {
        eprintln!("跳过 {dup} 条重复事件（同会话已入账）");
    }
    n
}

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(cli));
}

fn run(cli: Cli) -> i32 {
    match cli.cmd {
        Cmd::Demo { data_dir } => {
            let path = resolve_data_dir(&data_dir).join("ledger.jsonl");
            let now = Utc::now();
            let samples: [(&str, &str, Option<&str>, &str, u64, u64, u64, u64); 3] = [
                ("claude-code", "acme", Some("ACME 公司"), "claude-sonnet-4-5", 4_000_000, 600_000, 0, 0),
                ("codex", "billing", Some("ACME 计费项目"), "deepseek-chat", 2_000_000, 400_000, 2_000, 0),
                ("dsh", "side-project", None, "deepseek-chat", 900_000, 300_000, 800, 400),
            ];
            for (i, (tool, proj, client, model, tin, tout, rs, rf)) in samples.iter().enumerate() {
                let ev = ledger::LedgerEvent {
                    ts: (now - Duration::days(i as i64)).to_rfc3339(),
                    session_id: format!("demo-{i}"),
                    tool: (*tool).to_string(),
                    project: (*proj).to_string(),
                    client: (*client).map(|c| c.to_string()),
                    model: Some((*model).to_string()),
                    tokens_in: *tin,
                    tokens_out: *tout,
                    cache_read: 0,
                    cache_write: 0,
                    cost_native: 0.0,
                    currency: "CNY".to_string(),
                    requests_search: *rs,
                    requests_fetch: *rf,
                };
                if let Err(e) = ledger::append(&path, &ev) {
                    eprintln!("写入演示账本失败：{e}");
                    return 1;
                }
            }
            println!("已生成演示账本：{}", path.display());
            println!("接着跑：ebc ledger --days 7");
            0
        }

        Cmd::Ledger { days, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            let path = dir.join("ledger.jsonl");
            let (evs, bad) = ledger::read_all(&path).unwrap_or_else(|_| (Vec::new(), 0));
            if evs.is_empty() {
                eprintln!("账本为空或不存在（{}）。先跑 `ebc demo` 看效果。", path.display());
                return 1;
            }
            let cutoff = Utc::now()
                - Duration::days(days);
            let cutoff = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
            // 聚合键含 model 与 cache_read：否则费率表无法命中，金额恒为 ¥0
            #[derive(Default)]
            struct Agg {
                ti: u64,
                to: u64,
                cr: u64,
                cw: u64,
                sr: u64,
                sf: u64,
            }
            let mut agg: std::collections::BTreeMap<
                (String, String, String, String),
                Agg,
            > = Default::default();
            for e in &evs {
                if e.ts < cutoff {
                    continue;
                }
                let key = (
                    e.tool.clone(),
                    e.project.clone(),
                    e.client.clone().unwrap_or_else(|| "-".to_string()),
                    e.model.clone().unwrap_or_else(|| "-".to_string()),
                );
                let a = agg.entry(key).or_default();
                a.ti += e.tokens_in;
                a.to += e.tokens_out;
                a.cr += e.cache_read;
                a.cw += e.cache_write;
                a.sr += e.requests_search;
                a.sf += e.requests_fetch;
            }
            let table = load_rates(&dir);
            println!("近 {days} 天花费汇总（工具 × 项目 × 客户 × 模型）");
            println!(
                "{:<13} {:<16} {:<16} {:<18} {:>10} {:>9} {:>9} {:>7} {:>6} {:>8}",
                "tool", "project", "client", "model", "tok_in", "tok_out", "cache_rd", "search*", "fetch*", "≈¥"
            );
            let mut top: Vec<(f64, (String, String, String, String))> = Vec::new();
            for (k, a) in &agg {
                let est = rates::estimate(
                    &ledger::LedgerEvent {
                        ts: String::new(),
                        session_id: String::new(),
                        tool: k.0.clone(),
                        project: k.1.clone(),
                        client: None,
                        model: if k.3 == "-" { None } else { Some(k.3.clone()) },
                        tokens_in: a.ti,
                        tokens_out: a.to,
                        cache_read: a.cr,
                        cache_write: a.cw,
                        cost_native: 0.0,
                        currency: "CNY".to_string(),
                        requests_search: a.sr,
                        requests_fetch: a.sf,
                    },
                    &table,
                );
                top.push((est, k.clone()));
                println!(
                    "{:<13} {:<16} {:<16} {:<18} {:>10} {:>9} {:>9} {:>7} {:>6} {:>8.2}",
                    k.0, k.1, k.2, k.3, a.ti, a.to, a.cr, a.sr, a.sf, est
                );
            }
            // D8：首屏结论行
            top.sort_by(|a, b| b.0.total_cmp(&a.0));
            if let Some((c, k)) = top.first() {
                println!("\n花费最大：{} / {}（客户 {}，模型 {}），≈¥{:.2}", k.0, k.1, k.2, k.3, c);
            }
            if bad > 0 {
                eprintln!("（跳过了 {bad} 行无法解析的记录）");
            }
            if table.is_empty() {
                eprintln!("（未加载到费率表，金额只含已配置厂商；把 rates/*.yaml 放到 {}/rates）", dir.display());
            }
            0
        }

        Cmd::Ingest { claude_dir, codex_dir, dsh_dir, data_dir } => {
            let out = resolve_data_dir(&data_dir).join("ledger.jsonl");
            let mut n = 0usize;
            if let Some(root) = claude_dir {
                let ad = ClaudeCodeAdapter { session_hint: "cc".to_string() };
                n += ingest_dir(&root, &ad, &out);
            }
            if let Some(root) = codex_dir {
                let ad = CodexAdapter;
                n += ingest_dir(&root, &ad, &out);
            }
            // DSH：显式传入优先，否则自动探测 ~/.dsh/sessions
            let dsh_root = dsh_dir.or_else(|| {
                let d = home_dir().join(".dsh").join("sessions");
                d.exists().then_some(d)
            });
            if let Some(root) = dsh_root {
                let dir = resolve_data_dir(&data_dir);
                let ad = DshAdapter {
                    client_map: ebouncer_core::attrib::ClientMap::load(
                        &dir.join("config").join("client_map.yaml"),
                    ),
                };
                n += ingest_dir(&root, &ad, &out);
            }
            println!("已入账 {n} 条事件 → {}", out.display());
            0
        }

        Cmd::Bill { client, markup, days, format, out, trial_clean, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            let path = dir.join("ledger.jsonl");
            let (evs, _) = ledger::read_all(&path).unwrap_or_else(|_| (Vec::new(), 0));
            if evs.is_empty() {
                eprintln!("账本为空，先跑 ebc ingest");
                return 1;
            }
            let cutoff = (chrono::Utc::now() - Duration::days(days)).to_rfc3339();
            let cutoff = cutoff[..19].to_string();
            let evs: Vec<_> = evs.into_iter().filter(|e| e.ts >= cutoff).collect();
            let table = load_rates(&dir);

            // §6.2 次数级试用：前 3 张无水印，超出回落水印版并提示订阅
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
                    let _ = std::fs::write(
                        &trial_file,
                        format!("{{\"clean_invoices\": {used}}}"),
                    );
                    eprintln!("无水印试用额度：{used}/3 已用（超出后 invoice 带水印，订阅解锁不限量）");
                } else {
                    eprintln!("无水印额度已用完（3/3），本次带水印；订阅 Pro 解锁。");
                }
            }

            let basis = ledger::file_sha256(&path).ok();
            let inv = ebouncer_core::bill::build(&client, &evs, &table, markup, watermark, basis);
            if inv.items.is_empty() {
                eprintln!("客户 {client} 近 {days} 天无账目（检查归因：ebc ledger 里该 client 是否存在）");
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
                    println!("合计（含加成）：¥{:.2} · 估算未含税 · 账本指纹 {}", inv.total, &basis[..basis.len().min(12)]);
                }
            }
            if out {
                let exp = dir.join("exports");
                let _ = std::fs::create_dir_all(&exp);
                let stem = format!("invoice-{}-{}", client, chrono::Utc::now().format("%Y%m%d"));
                if matches!(format, BillFmt::Pdf) {
                    let html_p = unique_path(&exp, &stem, "html");
                    if std::fs::write(&html_p, ebouncer_core::render::invoice_html(&inv)).is_err() {
                        eprintln!("HTML 渲染失败");
                        return 1;
                    }
                    let pdf_p = unique_path(&exp, &stem, "pdf");
                    match ebouncer_core::render::html_to_pdf(&html_p, &pdf_p) {
                        Ok(_) => println!("已导出：{}\n      （源 HTML：{}）", pdf_p.display(), html_p.display()),
                        Err(e) => {
                            eprintln!("PDF 转换降级：{e}\n已导出：{}", html_p.display());
                        }
                    }
                    return 0;
                }
                let (ext, body): (&str, String) = match format {
                    BillFmt::Json => ("json", inv.to_json()),
                    BillFmt::Csv => ("csv", inv.to_csv()),
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
                Ok(0) => {
                    println!("审计链完好");
                    0
                }
                Ok(line) => {
                    eprintln!("第 {line} 行断链——审计可能被篡改");
                    2
                }
                Err(e) => {
                    eprintln!("校验失败：{e}");
                    1
                }
            }
        }

        Cmd::RulesCheck { rules: rules_path } => {
            let set = match rules::RuleSet::load(&rules_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("规则加载失败：{e}");
                    return 1;
                }
            };
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let cmd = line.trim();
                if cmd.is_empty() {
                    continue;
                }
                let (decision, id) = set.evaluate(cmd, true);
                println!("{decision:<9} {:<16} {cmd}", id.unwrap_or_else(|| "-".to_string()));
            }
            0
        }

        Cmd::Init { shell, yes } => hook::init(&home_dir(), match shell { ShellKind::Bash => "bash", ShellKind::Zsh => "zsh" }, yes),

        Cmd::Uninstall { purge, data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            hook::uninstall(&home_dir(), &dir, purge)
        }

        Cmd::Hook { data_dir } => {
            let dir = resolve_data_dir(&data_dir);
            hook::hook(&dir)
        }

        Cmd::Doctor => {
            println!("ebc doctor —— 环境自检（W6 补全签名/内核 BTF 检查）");
            let platform = match std::env::consts::OS {
                "windows" if std::env::var("MSYSTEM").is_ok() => {
                    "windows (MSYS2/类Unix shell)".to_string()
                }
                o => o.to_string(),
            };
            println!("  平台: {platform}");
            let dir = resolve_data_dir(&None);
            println!(
                "  数据目录: {} {}",
                dir.display(),
                if dir.exists() { "已存在" } else { "未初始化（跑 ebc demo）" }
            );
            for (name, p) in [
                ("DSH 日志", home_dir().join(".dsh").join("sessions")),
                ("Claude Code 日志", home_dir().join(".claude").join("projects")),
                ("Codex 日志", home_dir().join(".codex").join("sessions")),
            ] {
                println!("  {name}: {}", if p.exists() { "发现" } else { "未发现/未安装" });
            }
            for shell in ["bash", "zsh"] {
                let rc = hook::rc_path(&home_dir(), shell);
                let injected = std::fs::read_to_string(&rc)
                    .map(|s| s.contains(hook::BEGIN))
                    .unwrap_or(false);
                println!(
                    "  {shell} 钩子: {}",
                    if injected {
                        "已注入".to_string()
                    } else {
                        format!("未注入（跑 ebc init --shell {shell}）")
                    }
                );
            }
            let audit_p = dir.join("audit.jsonl");
            if audit_p.exists() {
                match audit::verify(&audit_p) {
                    Ok(0) => println!("  审计链: 完好"),
                    Ok(n) => println!("  审计链: ⛔ 第 {n} 行断链"),
                    Err(_) => println!("  审计链: 读取失败"),
                }
            } else {
                println!("  审计链: 尚无记录（钩子命中命令后生成）");
            }
            0
        }
    }
}
