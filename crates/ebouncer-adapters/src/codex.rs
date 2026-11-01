// SPDX-License-Identifier: AGPL-3.0-only
//! Codex CLI 会话日志解析（~/.codex/sessions/**/rollout-*.jsonl）。
//! TODO(W2): 字段路径以 golden 样本核验为准（token_count 事件、模型名、cwd）。

use super::{Adapter, normalize_project};
use ebouncer_core::ledger::LedgerEvent;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn parse_file(&self, path: &Path) -> Vec<LedgerEvent> {
        let Ok(f) = File::open(path) else { return Vec::new() };
        let mut out = Vec::new();
        let mut session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(sid) = v.pointer("/payload/id").and_then(Value::as_str) {
                session_id = sid.to_string();
            }
            // TODO 核验：token_count 事件携带 last_token_usage{input_tokens,output_tokens,cached_input_tokens}
            let Some(tc) = v.pointer("/payload/last_token_usage") else { continue };
            if v.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
                continue;
            }
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
                tokens_in: tc.get("input_tokens").and_then(Value::as_u64).unwrap_or_default(),
                tokens_out: tc.get("output_tokens").and_then(Value::as_u64).unwrap_or_default(),
                cache_read: tc
                    .get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                cost_native: 0.0,
                currency: "CNY".into(),
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
}
