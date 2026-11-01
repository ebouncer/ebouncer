// SPDX-License-Identifier: AGPL-3.0-only
//! ebc —— eBouncer CLI（W1 最小可用面：demo / ledger / doctor / audit-verify）。
//! 命令名刻意避开 `eb`（与 AWS EB CLI 冲突）。

use chrono::{Duration, Local};
use clap::{Parser, Subcommand};
use ebouncer_adapters::{Adapter, claude_code::ClaudeCodeAdapter, codex::CodexAdapter};
use ebouncer_core::{audit, ledger, rules};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ebc", version, about = "eBouncer —— OS-level bouncer for AI coding agents")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 用样例数据生成演示账本，跑通 ledger/bill 全流程
    Demo,
    /// 汇总近期账本：按工具×项目×客户
    Ledger {
        #[arg(long, default_value = "7")]
        days: i64,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// 扫描宿主日志 → 写入账本（W2 起接入真实解析）
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
    /// 规则自检：对 stdin 的每行命令输出 dry-run 决策
    RulesCheck {
        #[arg(long)]
        rules: PathBuf,
    },
    /// 环境自检（平台/权限/日志来源/费率表版本）
    Doctor,
}

fn data_dir(cli: &Option<PathBuf>) -> PathBuf {
    cli.clone().unwrap_or_else(|| {
        dirs_home().join(".ebouncer")
    })
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn main() {
    let cli = Cli::parse();
    let code = run(cli);
    std::process::exit(code);
}

fn run(cli: Cli) -> i32 {
    match cli.cmd {
        Cmd::Demo => {
            let dir = data_dir(&None);
            let path = dir.join("ledger.jsonl");
            let now = Local::now();
            let samples = [
                ("claude-code", "acme", Some("ACME 公司"), "claude-sonnet-4-5", 4_000_000u64, 600_000u64, 0u64, 0u64),
                ("codex", "billing", Some("ACME 计费项目"), "deepseek-chat", 2_000_000, 400_000, 2000, 0),
                ("dsh", "side-project", None, "deepseek-reasoner", 900_000, 300_000, 800, 0),
            ];
            for (i, (tool, proj, client, model, tin, tout, rs, rf)) in samples.iter().enumerate() {
                ledger::append(&path, &ledger::LedgerEvent {
                    ts: (now - Duration::days(i as i64)).to_rfc3339(),
                    session_id: format!("demo-{i}"),
                    tool: (*tool).into(),
                    project: (*proj).into(),
                    client: client.map(|c| c.to_string()),
                    model: Some((*model).into()),
                    tokens_in: *tin,
                    tokens_out: *tout,
                    cache_read: 0,
                    cost_native: 0.0,
                    currency: "CNY".into(),
                    requests_search: *rs,
                    requests_fetch: *rf,
                })
                .expect("write demo ledger");
            }
            println!("已生成演示账本：{}", path.display());
            println!("接着跑：ebc ledger --days 7");
            0
        }
        Cmd::Ledger { days, data_dir } => {
            let path = data_dir(&data_dir).join("ledger.jsonl");
            let (evs, bad) = ledger::read_all(&path)
                .map_err(|e| eprintln!("读取失败 {path:?}: {e}"))
                .unwrap_or_default();
            if evs.is_empty() {
                eprintln!("账本为空或不存在（{}）。先跑 `ebc demo` 看效果。", path.display());
                return 1;
            }
            let cutoff = (Local::now() - Duration::days(days)).to_rfc3339();
            let mut agg: std::collections::BTreeMap<(String, String, String), (u64, u64, u64, u64)> =
                Default::default();
            for e in &evs {
                if e.ts < cutoff { continue; }
                let k = (
                    e.tool.clone(),
                    e.project.clone(),
                    e.client.clone().unwrap_or_else(|| "-".into()),
                );
                let a = agg.entry(k).or_default();
                a.0 += e.tokens_in;
                a.1 += e.tokens_out;
                a.2 += e.requests_search;
                a.3 += e.requests_fetch;
            }
            println!("近 {days} 天花费汇总（工具 × 项目 × 客户）");
            println!("{:<13} {:<16} {:<16} {:>12} {:>12} {:>9} {:>7}", "tool", "project", "client", "tokens_in", "tokens_out", "search×", "fetch×");
            let table = ebouncer_core::rates::load_dir(&data_dir(&None).join("rates"));
            let mut top: Vec<(f64, (String, String, String))> = Vec::new();
            for (k, (ti, to, rs, rf)) in &agg {
                let est = ebouncer_core::rates::estimate(
                    &ledger::LedgerEvent {
                        ts: String::new(), session_id: String::new(), tool: k.0.clone(),
                        project: k.1.clone(), client: None, model: None,
                        tokens_in: *ti, tokens_out: *to, cache_read: 0,
                        cost_native: 0.0, currency: "CNY".into(),
                        requests_search: *rs, requests_fetch: *rf,
                    },
                    &table,
                );
                top.push((est, k.clone()));
                println!("{:<13} {:<16} {:<16} {:>12} {:>12} {:>9} {:>7}  ≈¥{:.2}",
                    k.0, k.1, k.2, ti, to, rs, rf, est);
            }
            // D8：首屏结论行
            top.sort_by(|a, b| b.0.total_cmp(&a.0));
            if let Some((c, k)) = top.first() {
                println!("\n💡 本月花费最大：{} / {}（客户 {}），≈¥{:.2}", k.0, k.1, k.2, c);
            }
            if bad > 0 {
                eprintln!("（跳过了 {bad} 行无法解析的记录）");
            }
            0
        }
        Cmd::Ingest { claude_dir, codex_dir, data_dir } => {
            let dir = data_dir(&data_dir);
            let out = dir.join("ledger.jsonl");
            let mut n = 0usize;
            for (root, ad) in [(claude_dir, &ClaudeCodeAdapter { session_hint: "cc".into() } as &dyn Adapter),
                               (codex_dir, &CodexAdapter as &dyn Adapter)] {
                let Some(root) = root else { continue };
                if let Ok(rd) = std::fs::read_dir(root) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                            for ev in ad.parse_file(&p) {
                                ledger::append(&out, &ev).ok();
                                n += 1;
                            }
                        }
                    }
                }
            }
            println!("已入账 {n} 条事件 → {}", out.display());
            0
        }
        Cmd::AuditVerify { data_dir } => {
            let p = data_dir(&data_dir).join("audit.jsonl");
            match audit::verify(&p) {
                Ok(0) => { println!("✅ 审计链完好"); 0 }
                Ok(line) => { eprintln!("⛔ 第 {line} 行断链——审计可能被篡改"); 2 }
                Err(e) => { eprintln!("校验失败：{e}"); 1 }
            }
        }
        Cmd::RulesCheck { rules } => {
            let rs = rules::RuleSet::load(&rules).unwrap_or_else(|e| {
                eprintln!("规则加载失败：{e}");
                std::process::exit(1)
            });
            let mut buf = String::new();
            while std::io::BufRead::read_line(&mut std::io::stdin(), &mut buf).is_ok() && !buf.is_empty() {
                let cmd = buf.trim();
                if cmd.is_empty() { break; }
                let (d, id) = rs.evaluate(cmd, true);
                println!("{d:<8} {:<14} {cmd}", id.unwrap_or_else(|| "-".into()));
                buf.clear();
            }
            0
        }
        Cmd::Doctor => {
            println!("ebc doctor —— 环境自检（W6 补全）");
            println!("  平台: {} (Linux 支持 probe 通道：内核 BTF 检查待接入)", std::env::consts::OS);
            let dir = data_dir(&None);
            println!("  数据目录: {} {}", dir.display(), if dir.exists() { "✅" } else { "（未初始化：跑 ebc demo）" });
            for (name, p) in [
                ("Claude Code 日志", dirs_home().join(".claude/projects")),
                ("Codex 日志", dirs_home().join(".codex/sessions")),
            ] {
                println!("  {name}: {}", if p.exists() { "✅ 发现" } else { "— 未安装/未发现" });
            }
            0
        }
    }
}
