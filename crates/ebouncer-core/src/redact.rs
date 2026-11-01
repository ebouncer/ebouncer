// SPDX-License-Identifier: AGPL-3.0-only
//! 落盘前脱敏（D3）：secret 命中项替换为哈希引用，明文永不进审计链/导出件。

use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

fn secret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r##"(?i)\b(api[_-]?key|token|secret|password|passwd|authorization|access[_-]?key|private[_-]?key)(\s*[:=]\s*)((?:bearer|basic)\s+)?([^\s'"]+)"##,
        )
        .unwrap()
    })
}

/// 返回脱敏后的命令文本。命中值替换为其 sha256 前 12 位引用；Bearer/Basic 前缀保留。
pub fn redact(cmd: &str) -> String {
    let re = secret_re();
    let mut out = String::with_capacity(cmd.len());
    let mut last = 0usize;
    for cap in re.captures_iter(cmd) {
        let whole = cap.get(0).unwrap();
        let key = cap.get(1).unwrap();
        let sep = cap.get(2).unwrap();
        let scheme = cap.get(3);
        let val = cap.get(4).unwrap();
        let mut h = Sha256::new();
        h.update(val.as_str().as_bytes());
        let digest: String = h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
        out.push_str(&cmd[last..key.end()]);
        out.push_str(&cmd[key.end()..sep.end()]);
        if let Some(s) = scheme {
            out.push_str(s.as_str());
        }
        out.push_str(&format!("<redacted sha256:{digest}>"));
        last = whole.end();
    }
    out.push_str(&cmd[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_key_redacts_value() {
        let r = redact("export API_KEY=sk-abcdef123456");
        assert!(r.contains("API_KEY="));
        assert!(!r.contains("sk-abcdef123456"));
        assert!(r.contains("<redacted sha256:"));
    }
    #[test]
    fn redacts_bearer_scheme() {
        let r = redact("curl -H Authorization: Bearer ghp_secrettoken");
        assert!(!r.contains("ghp_secrettoken"));
        assert!(r.contains("Bearer"));
    }
    #[test]
    fn passthrough_clean() {
        assert_eq!(redact("cargo test --workspace"), "cargo test --workspace");
    }
}
