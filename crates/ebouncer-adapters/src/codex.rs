// SPDX-License-Identifier: AGPL-3.0-only
//! Codex CLI 会话日志解析（~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl）。
//! 契约（2026-11-02 经 openai/codex 仓库 issue 区核验）：
//! - `payload.type=="token_count"` 的 `last_token_usage` 为单轮增量，`total_token_usage` 为累计
//!   （issue #22354）；取增量求和。
//! - 仅 rate-limit 变化时也会重发 TokenCount 且携带【与上次相同】的非零 last_token_usage
//!   （issue #14489）——连续相同增量必须跳过，否则重复计费。

use super::{Adapter, normalize_project};
use ebouncer_core::ledger::LedgerEvent;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn parse_file(&self, path: &Path) -> Vec<LedgerEvent> {
        let Ok(f) = File::open(path) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        // 上一条 token_count 的增量，用于识别 issue #14489 的重发
        let mut last_usage: (u64, u64, u64) = (u64::MAX, u64::MAX, u64::MAX);
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(sid) = v.pointer("/payload/id").and_then(Value::as_str) {
                session_id = sid.to_string();
            }
            let Some(tc) = v.pointer("/payload/last_token_usage") else {
                continue;
            };
            if v.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
                continue;
            }
            let key = (
                tc.get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                tc.get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                tc.get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            );
            if key == last_usage {
                continue; // rate-limit 重发，非新增消耗
            }
            last_usage = key;
            out.push(LedgerEvent {
                ts: v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or("1970-01-01T00:00:00Z")
                    .to_string(),
                session_id: session_id.clone(),
                tool: "codex".into(),
                project: normalize_project(
                    v.get("cwd").and_then(Value::as_str).unwrap_or("unknown"),
                ),
                client: None,
                model: v
                    .pointer("/payload/model")
                    .and_then(Value::as_str)
                    .map(String::from),
                tokens_in: tc
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                tokens_out: tc
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                cache_read: tc
                    .get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                cache_write: 0,
                cost_native: 0.0,
                currency: "USD".into(),
                requests_search: 0,
                requests_fetch: 0,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_stub_golden() {
        // TODO(W2): 换成真实脱敏样本
        let raw = r#"{"timestamp":"2026-11-01T08:00:00Z","type":"event_msg","payload":{"type":"token_count","model":"deepseek-v3","last_token_usage":{"input_tokens":1000,"output_tokens":200,"cached_input_tokens":0}},"cwd":"/home/u/proj/billing"}"#;
        let p = std::env::temp_dir().join("ebc-cx-stub.jsonl");
        std::fs::write(&p, raw).unwrap();
        let evs = CodexAdapter.parse_file(&p);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].project, "billing");
        std::fs::remove_file(&p).ok();
    }
    #[test]
    fn skips_resent_identical_token_count() {
        // issue #14489：rate-limit 变化会重发 token_count，last_token_usage 与上次相同
        let line = r#"{"timestamp":"2026-11-01T08:00:00Z","type":"event_msg","payload":{"type":"token_count","model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0}},"cwd":"/home/u/p"}"#;
        let p = std::env::temp_dir().join(format!("ebc-cx-dup-{}.jsonl", std::process::id()));
        std::fs::write(&p, format!("{line}\n{line}\n{line}\n")).unwrap();
        let evs = CodexAdapter.parse_file(&p);
        assert_eq!(evs.len(), 1, "重复重发只应计一次");
        std::fs::remove_file(&p).ok();
    }
}
