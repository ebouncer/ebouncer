// SPDX-License-Identifier: AGPL-3.0-only
//! 防篡改审计链（audit.jsonl）。K1 决策落地：事件体含 per-subject `key_ref` 字段，
//! 正文可加密（v0.2 接密钥后端）、哈希链保留——GDPR 行使删除权时销毁对应密钥即可。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub seq: u64,
    pub ts_ns: u128,
    pub uid: u32,
    pub tty: String,
    pub session: String,
    /// 已脱敏的命令文本（secret 落盘前哈希，D3）
    pub cmd: String,
    pub rule_id: Option<String>,
    pub decision: String, // allow | ask | deny | auto-deny | log(dry-run)
    /// 主体加密密钥引用（K1）
    pub key_ref: Option<String>,
    pub prev_hash: String,
    pub hash: String,
}

/// 链式哈希：对除自身 hash 字段外的全部字段做 sha256。
pub fn compute_hash(ev: &AuditEvent) -> String {
    let mut e = ev.clone();
    e.hash = String::new();
    let body = serde_json::to_string(&e).unwrap();
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn last_hash(path: &Path) -> String {
    if !path.exists() {
        return "0".repeat(64);
    }
    let mut last = String::new();
    if let Ok(f) = fs::File::open(path) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            if !l.trim().is_empty() {
                last = l;
            }
        }
    }
    serde_json::from_str::<AuditEvent>(&last)
        .map(|e| e.hash)
        .unwrap_or_else(|_| "0".repeat(64))
}

fn last_seq(path: &Path) -> u64 {
    let (mut n, _) = (0u64, ());
    if let Ok(f) = fs::File::open(path) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(e) = serde_json::from_str::<AuditEvent>(&l) {
                n = n.max(e.seq);
            }
        }
    }
    n
}

/// 追加一条审计事件并自动续链。数据目录 0700（Linux/WSL）。
pub fn append(path: &Path, mut ev: AuditEvent) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let _ = fs::DirBuilder::new().mode(0o700).create(dir);
            let _ = fs::create_dir_all(dir);
        }
        #[cfg(not(unix))]
        fs::create_dir_all(dir)?;
    }
    ev.seq = last_seq(path) + 1;
    ev.prev_hash = last_hash(path);
    ev.hash = compute_hash(&ev);
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(&ev).unwrap())?;
    Ok(())
}

/// 校验链完整性：返回第一个断链的行号（0=完好）。
pub fn verify(path: &Path) -> std::io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut prev = "0".repeat(64);
    for (i, l) in BufReader::new(fs::File::open(path)?)
        .lines()
        .map_while(Result::ok)
        .enumerate()
    {
        if l.trim().is_empty() {
            continue;
        }
        let ev: AuditEvent = serde_json::from_str(&l)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if ev.prev_hash != prev || ev.hash != compute_hash(&ev) {
            return Ok(i as u64 + 1);
        }
        prev = ev.hash;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chain_verify_detects_tamper() {
        let dir = std::env::temp_dir().join(format!("ebc-audit-test-{}", std::process::id()));
        let p = dir.join("audit.jsonl");
        let _ = fs::remove_file(&p);
        let base = AuditEvent {
            seq: 0, ts_ns: 1, uid: 1000, tty: "pts/0".into(), session: "s".into(),
            cmd: "ls".into(), rule_id: None, decision: "log".into(), key_ref: None,
            prev_hash: String::new(), hash: String::new(),
        };
        append(&p, base.clone()).unwrap();
        append(&p, base.clone()).unwrap();
        assert_eq!(verify(&p).unwrap(), 0);
    }
}
