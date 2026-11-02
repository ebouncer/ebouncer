// SPDX-License-Identifier: AGPL-3.0-only
//! 账本事件流（ledger.jsonl）：AI 花费的"唯一事实源"。
//! 注意：requests_search / requests_fetch 按次维度自第一天存在——
//! 2026-11-01 起 DeepSeek Search/Fetch 按次计费，账本必须记得住。

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEvent {
    pub ts: String, // ISO8601, UTC
    pub session_id: String,
    pub tool: String, // claude-code | codex | dsh | manual
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cache_read: u64,
    /// 缓存写入 token（DSH/Anthropic 类日志含此维度；隐式缓存厂商通常为 0）
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cost_native: f64,
    #[serde(default = "default_currency")]
    pub currency: String, // CNY | USD
    /// 工具按次维度：Web Search 成功请求数
    #[serde(default)]
    pub requests_search: u64,
    /// 工具按次维度：Fetch 成功请求数
    #[serde(default)]
    pub requests_fetch: u64,
}

fn default_currency() -> String {
    "CNY".to_string()
}

pub fn append(path: &Path, ev: &LedgerEvent) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(ev).unwrap())?;
    Ok(())
}

/// 宽松读取：坏行跳过并计数返回。
pub fn read_all(path: &Path) -> std::io::Result<(Vec<LedgerEvent>, u64)> {
    let mut out = Vec::new();
    let mut bad = 0u64;
    if !path.exists() {
        return Ok((out, bad));
    }
    let f = BufReader::new(fs::File::open(path)?);
    for line in f.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LedgerEvent>(&line) {
            Ok(ev) => out.push(ev),
            Err(_) => bad += 1,
        }
    }
    Ok((out, bad))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_skips_optional_fields() {
        let ev = LedgerEvent {
            ts: "2026-11-01T00:00:00Z".into(),
            session_id: "s1".into(),
            tool: "codex".into(),
            project: "p".into(),
            client: None,
            model: Some("deepseek-chat".into()),
            tokens_in: 10,
            tokens_out: 5,
            cache_read: 0,
            cache_write: 0,
            cost_native: 0.01,
            currency: "CNY".into(),
            requests_search: 2,
            requests_fetch: 0,
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(!j.contains("\"client\""));
        let back: LedgerEvent = serde_json::from_str(&j).unwrap();
        assert_eq!(back.requests_search, 2);
    }
}
