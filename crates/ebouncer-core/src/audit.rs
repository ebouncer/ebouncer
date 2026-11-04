// SPDX-License-Identifier: AGPL-3.0-only
//! 防篡改审计链（audit.jsonl）。K1 决策落地：事件体含 per-subject `key_ref` 字段，
//! 正文可加密（v0.2 接密钥后端）、哈希链保留——GDPR 行使删除权时销毁对应密钥即可。
//!
//! 完整性口径（诚实声明）：哈希链只能证明"文件内部自洽"，无密钥时同 UID 的攻击者可以
//! 重算整条链。因此额外维护 `audit.head` checkpoint（最后一条的 seq + hash）：
//! - 只删尾部记录 → 文件比 checkpoint 短，可检出；
//! - 删整个文件或删 checkpoint → checkpoint 缺失会显式降级告警；
//! - 同时改写正文并重算链、再更新 checkpoint → 本机无法防（需要外部锚点/内核探针）。
//! 这层保证的是"篡改可发现"，不是"不可篡改"。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

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

/// 审计链尾部锚点：最后一次 append 后的 (seq, hash)。
/// 与 audit.jsonl 分开存放，专门用来发现"只删尾部"这种最省事的篡改。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditCheckpoint {
    pub seq: u64,
    pub hash: String,
}

/// 校验结果。`is_ok()` 为假即应视为审计不可信。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// 校验过的条目数（忽略空行）
    pub lines: u64,
    /// 第一个断链行号（1-based）；0 = 链自洽
    pub broken_at: u64,
    /// checkpoint 与文件不一致时的 (期望条数, 实际条数)；None = 一致
    pub truncated: Option<(u64, u64)>,
    /// 是否读到并校验了 checkpoint（false 时无法检测尾部截断）
    pub checkpoint_checked: bool,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.broken_at == 0 && self.truncated.is_none()
    }
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

/// 尾部读取最后一行（窗口 64KB，覆盖正常审计行）。
fn tail_line(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let window = (len as usize).min(65536);
    let mut buf = vec![0u8; window];
    f.seek(SeekFrom::End(-(window as i64))).ok()?;
    f.read_exact(&mut buf).ok()?;
    // 窗口止于 EOF：取窗口内最后一条非空行即为末行
    let s = String::from_utf8_lossy(&buf);
    let trimmed = s.trim_end_matches(['\n', '\r']);
    let start = trimmed.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let last = trimmed[start..].trim();
    if last.is_empty() {
        None
    } else {
        Some(last.to_string())
    }
}

/// 一次尾部读取同时拿 (max_seq, last_hash)。
pub fn tail_info(path: &Path) -> (u64, String) {
    match path.exists().then(|| tail_line(path)).flatten() {
        Some(l) => serde_json::from_str::<AuditEvent>(&l)
            .map(|e| (e.seq, e.hash))
            .unwrap_or((0, "0".repeat(64))),
        None => (0, "0".repeat(64)),
    }
}

/// checkpoint 与审计文件同目录：audit.jsonl → audit.head。
pub fn checkpoint_path(path: &Path) -> PathBuf {
    path.with_file_name("audit.head")
}

/// 原子写入 checkpoint（先写 .tmp 再 rename，避免半截文件被当成篡改）。
fn write_checkpoint(path: &Path, seq: u64, hash: &str) -> std::io::Result<()> {
    let cp = AuditCheckpoint { seq, hash: hash.to_string() };
    let p = checkpoint_path(path);
    let tmp = p.with_extension("head.tmp");
    fs::write(&tmp, serde_json::to_vec(&cp)?)?;
    fs::rename(&tmp, &p)
}

/// 读取 checkpoint；文件不存在返回 None，内容损坏返回 Err（可能是篡改，也可能是磁盘损坏）。
fn read_checkpoint(path: &Path) -> std::io::Result<Option<AuditCheckpoint>> {
    let p = checkpoint_path(path);
    match fs::read(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
        Ok(bytes) => serde_json::from_slice::<AuditCheckpoint>(&bytes)
            .map(Some)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    }
}

/// 追加一条审计事件并自动续链。数据目录 0700（Linux/WSL）。
/// 先写正文、再原子更新 checkpoint：中途崩溃只会留下"文件比锚点多几条"，不会误报截断。
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
    let (seq, prev) = tail_info(path);
    ev.seq = seq + 1;
    ev.prev_hash = prev;
    ev.hash = compute_hash(&ev);
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(&ev).unwrap())?;
    f.flush()?;
    write_checkpoint(path, ev.seq, &ev.hash)?;
    Ok(())
}

/// 扫描审计文件：返回 (有效条目数, 断链行号或 0, 命中 watch_seq 的那条的 hash)。
fn scan(path: &Path, watch_seq: Option<u64>) -> std::io::Result<(u64, u64, Option<String>)> {
    let mut prev = "0".repeat(64);
    let mut parsed = 0u64;
    let mut hash_at = None;
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
        parsed += 1;
        if ev.prev_hash != prev || ev.hash != compute_hash(&ev) {
            return Ok((parsed, i as u64 + 1, hash_at));
        }
        prev = ev.hash.clone();
        if watch_seq == Some(ev.seq) {
            hash_at = Some(ev.hash);
        }
    }
    Ok((parsed, 0, hash_at))
}

/// 校验链完整性 + 尾部截断（checkpoint 对比）。
/// 断链返回 broken_at；尾部被删返回 truncated；无 checkpoint 时只能校验链内自洽。
pub fn verify(path: &Path) -> std::io::Result<VerifyReport> {
    let checkpoint = read_checkpoint(path)?;
    let mut report = VerifyReport {
        lines: 0,
        broken_at: 0,
        truncated: None,
        checkpoint_checked: checkpoint.is_some(),
    };
    if !path.exists() {
        if let Some(cp) = checkpoint {
            report.truncated = Some((cp.seq, 0));
        }
        return Ok(report);
    }
    let (lines, broken_at, hash_at) = scan(path, checkpoint.as_ref().map(|c| c.seq))?;
    report.lines = lines;
    report.broken_at = broken_at;
    if let Some(cp) = checkpoint {
        if broken_at == 0 {
            match hash_at {
                // 文件比锚点短：尾部被删
                None => report.truncated = Some((cp.seq, lines)),
                // 同序号但内容/哈希不同：该条被替换
                Some(h) if h != cp.hash => report.truncated = Some((cp.seq, lines)),
                _ => {}
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AuditEvent {
        AuditEvent {
            seq: 0, ts_ns: 1, uid: 1000, tty: "pts/0".into(), session: "s".into(),
            cmd: "ls".into(), rule_id: None, decision: "log".into(), key_ref: None,
            prev_hash: String::new(), hash: String::new(),
        }
    }

    /// 每个测试用独立目录，避免并行测试互相删文件。
    fn tmp(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = std::env::temp_dir().join(format!("ebc-audit-{name}-{}-{n}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        d
    }

    #[test]
    fn chain_ok_and_checkpoint_checked() {
        let p = tmp("ok").join("audit.jsonl");
        append(&p, base()).unwrap();
        append(&p, base()).unwrap();
        let r = verify(&p).unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(r.lines, 2);
        assert!(r.checkpoint_checked);
    }

    /// 回归（2026-11-04 实测）：删掉尾部一条，旧实现仍报"审计链完好"。
    #[test]
    fn detects_tail_truncation() {
        let p = tmp("trunc").join("audit.jsonl");
        for _ in 0..3 {
            append(&p, base()).unwrap();
        }
        let body = fs::read_to_string(&p).unwrap();
        let mut lines: Vec<&str> = body.lines().collect();
        lines.pop();
        fs::write(&p, format!("{}\n", lines.join("\n"))).unwrap();
        let r = verify(&p).unwrap();
        assert!(!r.is_ok(), "{r:?}");
        assert_eq!(r.truncated, Some((3, 2)));
        assert_eq!(r.broken_at, 0);
    }

    /// 整个审计文件被删，但锚点还在：同样算截断。
    #[test]
    fn detects_whole_file_deletion() {
        let p = tmp("rm").join("audit.jsonl");
        append(&p, base()).unwrap();
        fs::remove_file(&p).unwrap();
        let r = verify(&p).unwrap();
        assert_eq!(r.truncated, Some((1, 0)));
    }

    /// checkpoint 缺失时明确降级为"只校验链"，不能假装校验过截断。
    #[test]
    fn missing_checkpoint_reports_unchecked() {
        let p = tmp("nocp").join("audit.jsonl");
        append(&p, base()).unwrap();
        fs::remove_file(checkpoint_path(&p)).unwrap();
        let r = verify(&p).unwrap();
        assert!(r.is_ok());
        assert!(!r.checkpoint_checked);
    }

    #[test]
    fn detects_field_tamper() {
        let p = tmp("tamper").join("audit.jsonl");
        append(&p, base()).unwrap();
        let body = fs::read_to_string(&p).unwrap().replace("\"ls\"", "\"rm -rf ~\"");
        fs::write(&p, body).unwrap();
        let r = verify(&p).unwrap();
        assert!(!r.is_ok(), "{r:?}");
        assert_eq!(r.broken_at, 1);
    }
}
