// SPDX-License-Identifier: AGPL-3.0-only
//! ebc —— eBouncer CLI（W1 最小可用面：demo / ledger / ingest / audit-verify / rules-check / doctor）。
//! 命令名刻意避开 `eb`（与 AWS EB CLI 冲突）。

use chrono::{Duration, Local};
use clap::{Parser, Subcommand};
use ebouncer_adapters::{Adapter, claude_code::ClaudeCodeAdapter, codex::CodexAdapter};
use ebouncer_core::{audit, ledger, rates, rules};
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
    Demo,
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
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 审计链完整性校验
    AuditVerify {
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

fn ingest_dir(root: &Path, ad: &dyn Adapter, out: &Path) -> usize {
    let mut n = 0usize;
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                for ev in ad.parse_file(&p) {
                    if ledger::append(out, &ev).is_ok() {
                        n += 1;
                    }
                }
            }
        }
    }
    n
}

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(cli));
}

fn run(cli: Cli) -> i32 {
    match cli.cmd {
        Cmd::Demo => {
            let path = resolve_data_dir(&None).join("ledger.jsonl");
            let now = Local::now();
            let samples: [(&str, &str, Option<&str>, &str, u64, u64, u64, u64); 3] = [
                ("claude-code", "acme", Some("ACME 公司"), "claude-sonnet-4-5", 4_000_000, 600_000, 0, 0),
                ("codex", "billing", Some("ACME 计费项目"), "deepseek-chat", 2_000_000, 400_000, 2_000, 0),
                ("dsh", "side-project", None, "deepseek-reasoner", 900_000, 300_000, 800, 400),
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
            let (evs, bad) = ledger::read_all(&path).unwrap_or_default();
            if evs.is_empty() {
                eprintln!("账本为空或不存在（{}）。先跑 `ebc demo` 看效果。", path.display());
                return 1;
            }
            let cutoff = (Local::now() - Duration::days(days)).to_rfc3339();
            let mut agg: std::collections::BTreeMap<
                (String, String, String),
                (u64, u64, u64, u64),
            > = Default::default();
            for e in &evs {
                if e.ts < cutoff {
                    continue;
                }
                let key = (
                    e.tool.clone(),
                    e.project.clone(),
                    e.client.clone().unwrap_or_else(|| "-".to_string()),
                );
                let a = agg.entry(key).or_default();
                a.0 += e.tokens_in;
                a.1 += e.tokens_out;
                a.2 += e.requests_search;
                a.3 += e.requests_fetch;
            }
            let table = rates::load_dir(&dir.join("rates"));
            println!("近 {days} 天花费汇总（工具 × 项目 × 客户）");
            println!(
                "{:<13} {:<16} {:<18} {:>11} {:>11} {:>8} {:>7}",
                "tool", "project", "client", "tokens_in", "tokens_out", "search*", "fetch*"
            );
            let mut top: Vec<(f64, (String, String, String))> = Vec::new();
            for (k, (ti, to, rs, rf)) in &agg {
                let est = rates::estimate(
                    &ledger::LedgerEvent {
                        ts: String::new(),
                        session_id: String::new(),
                        tool: k.0.clone(),
                        project: k.1.clone(),
                        client: None,
                        model: None,
                        tokens_in: *ti,
                        tokens_out: *to,
                        cache_read: 0,
                        cost_native: 0.0,
                        currency: "CNY".to_string(),
                        requests_search: *rs,
                        requests_fetch: *rf,
                    },
                    &table,
                );
                top.push((est, k.clone()));
                println!(
                    "{:<13} {:<16} {:<18} {:>11} {:>11} {:>8} {:>7}  ≈¥{:.2}",
                    k.0, k.1, k.2, ti, to, rs, rf, est
                );
            }
            // D8：首屏结论行
            top.sort_by(|a, b| b.0.total_cmp(&a.0));
            if let Some((c, k)) = top.first() {
                println!("\n本月花费最大：{} / {}（客户 {}），≈¥{:.2}", k.0, k.1, k.2, c);
            }
            if bad > 0 {
                eprintln!("（跳过了 {bad} 行无法解析的记录）");
            }
            if table.is_empty() {
                eprintln!("（未加载到费率表，金额只含已配置厂商；把 rates/*.yaml 放到 {}/rates）", dir.display());
            }
            0
        }

        Cmd::Ingest { claude_dir, codex_dir, data_dir } => {
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
            println!("已入账 {n} 条事件 → {}", out.display());
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

        Cmd::Doctor => {
            println!("ebc doctor —— 环境自检（W6 补全签名/内核 BTF 检查）");
            println!("  平台: {}", std::env::consts::OS);
            let dir = resolve_data_dir(&None);
            println!(
                "  数据目录: {} {}",
                dir.display(),
                if dir.exists() { "已存在" } else { "未初始化（跑 ebc demo）" }
            );
            for (name, p) in [
                ("Claude Code 日志", home_dir().join(".claude").join("projects")),
                ("Codex 日志", home_dir().join(".codex").join("sessions")),
            ] {
                println!("  {name}: {}", if p.exists() { "发现" } else { "未发现/未安装" });
            }
            0
        }
    }
}
