// SPDX-License-Identifier: AGPL-3.0-only
//! Claude Code 会话日志解析（~/.claude/projects/<编码项目路径>/<sessionId>.jsonl）。
//! 契约（2026-11-02 经 ccusage/格式文档/Anthropic prompt-caching 文档三方核验）：
//! - `type=="assistant"` 行、`message.usage{input_tokens, output_tokens,
//!   cache_read_input_tokens, cache_creation_input_tokens}`；顶层 `sessionId`/`cwd`/`timestamp`。
//! - 同一条 API 响应可能被写成多行 assistant（共享 `message.id`）；按
//!   `(sessionId, message.id)` 去重，token 取各字段最大值，避免重复入账。
//! - `usage.server_tool_use.web_search_requests / web_fetch_requests` 计入按次维度；
//!   Anthropic Web Search 单独按次收费，Web Fetch 只计 token。
//!
//! 已知上游风险（读到时留意，不静默）：anthropics/claude-code#28197 / #25941 报告个别版本
//! JSONL 把 input/output_tokens 写成流式占位值(≈1)。若发现异常小值，ledger 会原样入账——
//! 校准建议对照 `npx ccusage` 同期数字（W4 兼容套件加一条对账脚本）。

use super::{Adapter, normalize_project};
use ebouncer_core::ledger::LedgerEvent;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct ClaudeCodeAdapter {
    pub session_hint: String,
}

impl Adapter for ClaudeCodeAdapter {
    fn parse_file(&self, path: &Path) -> Vec<LedgerEvent> {
        let Ok(f) = File::open(path) else {
            return Vec::new();
        };
        let mut out: Vec<LedgerEvent> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(usage) = v.pointer("/message/usage") else {
                continue;
            };
            let session = v
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or(&self.session_hint)
                .to_string();
            let ev = LedgerEvent {
                ts: v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or("1970-01-01T00:00:00Z")
                    .to_string(),
                session_id: session.clone(),
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
                requests_search: usage
                    .pointer("/server_tool_use/web_search_requests")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                requests_fetch: usage
                    .pointer("/server_tool_use/web_fetch_requests")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            };

            let msg_id = v
                .pointer("/message/id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if msg_id.is_empty() {
                out.push(ev);
                continue;
            }
            let key = format!("{session}\u{0}{msg_id}");
            if let Some(&i) = seen.get(&key) {
                merge_max(&mut out[i], &ev);
            } else {
                seen.insert(key, out.len());
                out.push(ev);
            }
        }
        out
    }
}

/// 同一条响应的多行是重复计费，不是增量：每个字段取最大值。
fn merge_max(a: &mut LedgerEvent, b: &LedgerEvent) {
    a.tokens_in = a.tokens_in.max(b.tokens_in);
    a.tokens_out = a.tokens_out.max(b.tokens_out);
    a.cache_read = a.cache_read.max(b.cache_read);
    a.cache_write = a.cache_write.max(b.cache_write);
    a.requests_search = a.requests_search.max(b.requests_search);
    a.requests_fetch = a.requests_fetch.max(b.requests_fetch);
    if b.ts < a.ts {
        a.ts = b.ts.clone();
    }
    if a.model.is_none() {
        a.model = b.model.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_stub_golden() {
        let raw = r#"{"type":"assistant","timestamp":"2026-11-01T08:00:00Z","sessionId":"abc","cwd":"/home/u/work/acme","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":120,"output_tokens":340,"cache_read_input_tokens":50}}}"#;
        let p = std::env::temp_dir().join("ebc-cc-stub.jsonl");
        std::fs::write(&p, raw).unwrap();
        let evs = ClaudeCodeAdapter {
            session_hint: "x".into(),
        }
        .parse_file(&p);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].project, "acme");
        assert_eq!(evs[0].tokens_in, 120);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn dedups_same_message_id_and_counts_server_tools() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-11-01T08:00:00Z","sessionId":"s1","cwd":"/w/acme","message":{"id":"msg_1","model":"claude-sonnet-4-5","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":5,"cache_creation_input_tokens":7,"server_tool_use":{"web_search_requests":1,"web_fetch_requests":1}}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-11-01T08:00:01Z","sessionId":"s1","cwd":"/w/acme","message":{"id":"msg_1","model":"claude-sonnet-4-5","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":5,"cache_creation_input_tokens":7,"server_tool_use":{"web_search_requests":1,"web_fetch_requests":1}}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-11-01T08:00:02Z","sessionId":"s1","cwd":"/w/acme","message":{"id":"msg_2","model":"claude-sonnet-4-5","usage":{"input_tokens":200,"output_tokens":20,"server_tool_use":{"web_search_requests":2}}}}"#,
        );
        let p = std::env::temp_dir().join("ebc-cc-dedup.jsonl");
        std::fs::write(&p, raw).unwrap();
        let evs = ClaudeCodeAdapter {
            session_hint: "x".into(),
        }
        .parse_file(&p);
        assert_eq!(evs.len(), 2, "共享 message.id 的多行应合并");
        assert_eq!(evs.iter().map(|e| e.tokens_in).sum::<u64>(), 300);
        assert_eq!(evs.iter().map(|e| e.tokens_out).sum::<u64>(), 30);
        assert_eq!(evs.iter().map(|e| e.cache_read).sum::<u64>(), 5);
        assert_eq!(evs.iter().map(|e| e.requests_search).sum::<u64>(), 3);
        assert_eq!(evs.iter().map(|e| e.requests_fetch).sum::<u64>(), 1);
        std::fs::remove_file(&p).ok();
    }
}
