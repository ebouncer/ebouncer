// SPDX-License-Identifier: AGPL-3.0-only
//! Claude Code 会话日志解析（~/.claude/projects/<slug>/*.jsonl）。
//! TODO(W2): 字段名以 golden 样本核验为准——usage 子对象路径、cwd、sessionId、model。

use super::{Adapter, normalize_project};
use ebouncer_core::ledger::LedgerEvent;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct ClaudeCodeAdapter {
    pub session_hint: String,
}

impl Adapter for ClaudeCodeAdapter {
    fn parse_file(&self, path: &Path) -> Vec<LedgerEvent> {
        let Ok(f) = File::open(path) else { return Vec::new() };
        let mut out = Vec::new();
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            // TODO 核验：assistant 消息携带 usage{input_tokens,output_tokens,cache_read_input_tokens}
            let usage = match v.pointer("/message/usage") {
                Some(u) => u,
                None => continue,
            };
            if v.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            out.push(LedgerEvent {
                ts: v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or("1970-01-01T00:00:00Z")
                    .to_string(),
                session_id: v
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or(&self.session_hint)
                    .to_string(),
                tool: "claude-code".into(),
                project: normalize_project(
                    v.get("cwd").and_then(Value::as_str).unwrap_or("unknown"),
                ),
                client: None,
                model: v
                    .pointer("/message/model")
                    .and_then(Value::as_str)
                    .map(String::from),
                tokens_in: usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                tokens_out: usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                cache_read: usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                cache_write: usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                cost_native: 0.0,
                currency: "USD".into(),
                requests_search: 0, // TODO 核验：WebSearch 工具事件出现次数→按次计数
                requests_fetch: 0,  // TODO 核验：WebFetch 工具事件出现次数→按次计数
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
        // TODO(W2): 换成 tests/golden/claude_code/session_sample.jsonl 真实脱敏样本
        let raw = r#"{"type":"assistant","timestamp":"2026-11-01T08:00:00Z","sessionId":"abc","cwd":"/home/u/work/acme","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":120,"output_tokens":340,"cache_read_input_tokens":50}}}"#;
        let p = std::env::temp_dir().join("ebc-cc-stub.jsonl");
        std::fs::write(&p, raw).unwrap();
        let evs = ClaudeCodeAdapter { session_hint: "x".into() }.parse_file(&p);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].project, "acme");
        assert_eq!(evs[0].tokens_in, 120);
        std::fs::remove_file(&p).ok();
    }
}
