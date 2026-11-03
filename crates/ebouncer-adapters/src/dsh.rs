// SPDX-License-Identifier: AGPL-3.0-only
//! DeepSeek Harness (DSH) 会话日志解析器。
//!
//! 契约来源：对真实 `~/.dsh/sessions/**/session.v4.jsonl.zstd` 逐字段核验（2026-11-02），
//! 关键事实——
//! - 文件为 zstd 压缩的 JSONL（扩展名 `.jsonl.zstd`）；也兼容未压缩 `.jsonl`。
//! - 首行 `type=="session"` 携带 `cwd` 与 `id`，用于项目归一化与会话号。
//! - `type=="assistant/message"` 的 `data.usage{inputTokens,outputTokens,cacheReadTokens,cacheWriteTokens,totalTokens}`
//!   为**单次增量**（已验证 inputTokens 不单调），逐个求和即正确记账。
//! - 模型名在 `data.message.source.model`（如 `deepseek-flash`），provider 同层。
//! - 顶层 `time` 为 13 位毫秒时间戳。
//! - `type=="tool/call"` 的 `data.name` 中命中 search/fetch 语义的计入按次维度。

use super::{Adapter, normalize_project};
use ebouncer_core::attrib::{self, ClientMap};
use ebouncer_core::ledger::LedgerEvent;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct DshAdapter {
    /// 客户归因映射表（CLI 注入；默认空 = 仅项目文件/git remote 生效）
    pub client_map: ClientMap,
}

impl Default for DshAdapter {
    fn default() -> Self {
        Self { client_map: ClientMap::default() }
    }
}

/// DSH 日志实测为「每行一个 zstd frame」的追加式文件（53 行/27 帧等），
/// 而 ruzstd StreamingDecoder 只解单帧——必须逐帧循环。
/// 帧边界：每帧长度自描述，解完一帧后 Cursor.position() 即下一帧起点。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

fn read_maybe_zstd(path: &Path) -> std::io::Result<String> {
    let mut buf = Vec::new();
    File::open(path)?.read_to_end(&mut buf)?;
    if !(buf.len() >= 4 && buf[0..4] == ZSTD_MAGIC) {
        return Ok(String::from_utf8_lossy(&buf).into_owned());
    }
    let mut out = Vec::new();
    let mut off = 0usize;
    loop {
        let start = match buf[off..].windows(4).position(|w| w == ZSTD_MAGIC) {
            Some(p) => off + p,
            None => break,
        };
        let mut cur = std::io::Cursor::new(&buf[start..]);
        let mut sd = ruzstd::decoding::StreamingDecoder::new(&mut cur)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let mut frame = Vec::new();
        sd.read_to_end(&mut frame)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let (reader, _) = sd.into_parts();
        let used = reader.position() as usize;
        if used == 0 {
            break;
        }
        out.extend(frame);
        off = start + used;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// 毫秒时间戳 → RFC3339 UTC。
fn ts_from_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// 判断 DSH 工具名是否属于联网搜索/抓取（按次计费维度）。
fn is_search_or_fetch(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    if n.contains("search") {
        Some("search")
    } else if n.contains("fetch") || n.contains("webread") {
        Some("fetch")
    } else {
        None
    }
}

/// 一条纯按次事件（token 全 0，仅 requests_* 计数），tool/call 分支用。
fn blank_event(session: &str, time_ms: Option<u64>) -> LedgerEvent {
    LedgerEvent {
        ts: time_ms.map(ts_from_ms).unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()),
        session_id: session.to_string(),
        tool: "dsh".to_string(),
        project: String::new(),
        client: None,
        model: None,
        tokens_in: 0,
        tokens_out: 0,
        cache_read: 0,
        cache_write: 0,
        cost_native: 0.0,
        currency: "CNY".to_string(),
        requests_search: 0,
        requests_fetch: 0,
    }
}

impl DshAdapter {
    /// 从路径解析出会话 id（供无 session 头时兜底）。
    fn session_from_path(path: &Path) -> String {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    }
}

impl Adapter for DshAdapter {
    fn parse_file(&self, path: &Path) -> Vec<LedgerEvent> {
        let Ok(text) = read_maybe_zstd(path) else {
            return Vec::new();
        };
        let mut project: Option<String> = None;
        let mut cwd_raw: Option<String> = None;
        let mut session = Self::session_from_path(path);
        let mut events: Vec<LedgerEvent> = Vec::new();
        let mut call_seq = 0u64;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            let time_ms = v.get("time").and_then(Value::as_u64);

            match ty {
                "session" => {
                    // 真实格式 cwd/id 在顶层；data.* 作兜底以兼容早期样例
                    if let Some(cwd) = v
                        .get("cwd")
                        .and_then(Value::as_str)
                        .or_else(|| v.pointer("/data/cwd").and_then(Value::as_str))
                    {
                        cwd_raw = Some(cwd.to_string());
                        project = Some(normalize_project(cwd));
                    }
                    if let Some(id) = v
                        .get("id")
                        .and_then(Value::as_str)
                        .or_else(|| v.pointer("/data/id").and_then(Value::as_str))
                    {
                        session = id.to_string();
                    }
                }
                "tool/call" => {
                    if let Some(name) = v.pointer("/data/name").and_then(Value::as_str) {
                        // 每个联网调用生成一条独立按次事件（ts 带 seq 保证幂等键唯一，
                        // 会话续写后的新增调用能被 ingest 增量捕获）
                        if let Some(kind) = is_search_or_fetch(name) {
                            call_seq += 1;
                            let mut ev = blank_event(&session, time_ms);
                            match kind {
                                "search" => ev.requests_search = 1,
                                _ => ev.requests_fetch = 1,
                            }
                            ev.session_id = format!("{session}#{call_seq}");
                            events.push(ev);
                        }
                    }
                }
                "assistant/message" => {
                    let u = &v["data"]["usage"];
                    events.push(LedgerEvent {
                        ts: time_ms.map(ts_from_ms).unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()),
                        session_id: session.clone(),
                        tool: "dsh".to_string(),
                        project: String::new(),
                        client: None,
                        model: v
                            .pointer("/data/message/source/model")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        tokens_in: u.get("inputTokens").and_then(Value::as_u64).unwrap_or_default(),
                        tokens_out: u.get("outputTokens").and_then(Value::as_u64).unwrap_or_default(),
                        cache_read: u.get("cacheReadTokens").and_then(Value::as_u64).unwrap_or_default(),
                        cache_write: u.get("cacheWriteTokens").and_then(Value::as_u64).unwrap_or_default(),
                        cost_native: 0.0,
                        currency: "CNY".to_string(),
                        requests_search: 0,
                        requests_fetch: 0,
                    });
                }
                _ => {}
            }
        }

        let project = project.unwrap_or_else(|| "unknown".to_string());
        // K2 归因链现场解析：cwd 作为"项目目录"跑 显式>项目文件>git remote
        let client = cwd_raw
            .as_deref()
            .map(PathBuf::from)
            .and_then(|dir| attrib::resolve_client(None, &dir, &self.client_map));
        for e in events.iter_mut() {
            e.project = project.clone();
            e.client = client.clone();
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 结构忠实的最小合成样本（非真实会话内容）。
    const GOLDEN: &str = concat!(
        r#"{"type":"session","seq":0,"time":1791340654300,"data":{"id":"session-abc","cwd":"E:\\proj\\billing","version":4}}"#,
        "\n",
        r#"{"type":"assistant/message","seq":1,"time":1791340660000,"data":{"usage":{"inputTokens":6189,"outputTokens":417,"cacheReadTokens":1024,"cacheWriteTokens":0,"totalTokens":7630},"message":{"source":{"provider":"deepseek-official","model":"deepseek-flash"}}}}"#,
        "\n",
        r#"{"type":"tool/call","seq":2,"time":1791340661000,"data":{"name":"websearch","arguments":"{}"}}"#,
        "\n",
        r#"{"type":"tool/call","seq":3,"time":1791340662000,"data":{"name":"fetch_url","arguments":"{}"}}"#,
        "\n",
        r#"{"type":"assistant/message","seq":4,"time":1791340663000,"data":{"usage":{"inputTokens":150,"outputTokens":120,"cacheReadTokens":7552,"cacheWriteTokens":0,"totalTokens":7822},"message":{"source":{"provider":"deepseek-official","model":"deepseek-flash"}}}}"#,
        "\n",
    );

    fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("ebc-dsh-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn parses_increment_usage_and_project() {
        let p = write_tmp("basic", GOLDEN);
        let evs = DshAdapter::default().parse_file(&p);
        // 2 条 assistant + 2 条按次（websearch/fetch_url）
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[0].project, "billing");
        assert_eq!(evs[0].session_id, "session-abc");
        assert_eq!(evs[0].model.as_deref(), Some("deepseek-flash"));
        assert_eq!(evs[0].tokens_in, 6189);
        assert_eq!(evs[0].cache_read, 1024);
        // 时间戳换算正确（毫秒 → UTC）
        assert!(evs[0].ts.starts_with("2026-"));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn per_request_counts_not_double_counted() {
        let p = write_tmp("req", GOLDEN);
        let evs = DshAdapter::default().parse_file(&p);
        let s: u64 = evs.iter().map(|e| e.requests_search).sum();
        let f: u64 = evs.iter().map(|e| e.requests_fetch).sum();
        assert_eq!(s, 1);
        assert_eq!(f, 1);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn handles_garbage_lines() {
        let p = write_tmp("garbage", "not json\n{}\n[1,2]\n");
        let evs = DshAdapter::default().parse_file(&p);
        assert!(evs.is_empty());
        std::fs::remove_file(&p).ok();
    }

    /// 回归：真实 DSH 日志是「每行一个 zstd frame」（每帧含行尾换行），只解首帧会丢全部后续行。
    #[test]
    fn decompresses_multiple_frames() {
        use ruzstd::encoding::{compress_to_vec, CompressionLevel};
        let mut blob = Vec::new();
        for line in GOLDEN.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            // 帧内容必须带换行，否则解压后多行 JSON 会粘连成一行
            blob.extend(compress_to_vec(format!("{t}\n").as_bytes(), CompressionLevel::Fastest));
        }
        let p = std::env::temp_dir().join(format!("ebc-dsh-frames-{}.zstd", std::process::id()));
        std::fs::write(&p, &blob).unwrap();
        let evs = DshAdapter::default().parse_file(&p);
        // 若只解首帧，这里最多得到 0~1 条事件；4 = 2 assistant + 2 按次
        assert_eq!(evs.len(), 4, "多帧文件应解析出全部事件");
        assert_eq!(evs[0].project, "billing");
        std::fs::remove_file(&p).ok();
    }
}
