// SPDX-License-Identifier: AGPL-3.0-only
//! 客户归因引擎（K2 决策）：显式 --client > 项目 .ebouncer/client > git remote 域名映射。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// remote 前缀 → 客户名 的映射表（config/client_map.yaml 加载）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientMap {
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
}

impl ClientMap {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_yaml::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// 最长前缀匹配（对归一化后的 remote 地址）
    fn lookup(&self, remote: &str) -> Option<String> {
        let mut best: Option<(usize, String)> = None;
        for (k, v) in &self.rules {
            if remote.starts_with(k.as_str()) {
                let n = k.len();
                if best.as_ref().map(|(m, _)| n > *m).unwrap_or(true) {
                    best = Some((n, v.clone()));
                }
            }
        }
        best.map(|(_, v)| v)
    }
}

/// 从 .git/config 里抠出第一个 remote url（够用且零依赖 git CLI）。
fn first_remote_url(project_dir: &Path) -> Option<String> {
    let cfg = fs::read_to_string(project_dir.join(".git").join("config")).ok()?;
    let mut in_remote = false;
    for line in cfg.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_remote = t.starts_with("[remote ");
        } else if in_remote
            && t.starts_with("url")
            && let Some((_, v)) = t.split_once('=')
        {
            return Some(normalize_remote(v.trim()));
        }
    }
    None
}

/// git@github.com:acme/billing.git / https://github.com/acme/billing.git → github.com/acme/billing
fn normalize_remote(u: &str) -> String {
    let mut s = u.trim().to_string();
    for p in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.to_string();
            break;
        }
    }
    if let Some(at) = s.find('@') {
        s = s[at + 1..].to_string();
    }
    if let Some(colon) = s.find(':') {
        let tail = &s[colon + 1..];
        // scp 语法 host:path → host/path；host:22 这类纯数字端口不转换
        if !tail.is_empty() && !tail.chars().next().unwrap_or('0').is_ascii_digit() {
            s = format!("{}/{}", &s[..colon], tail);
        }
    }
    s.trim_end_matches('/').trim_end_matches(".git").to_string()
}

/// K2 优先级链：explicit > 项目标记文件 > git remote 映射。
pub fn resolve_client(
    explicit: Option<&str>,
    project_dir: &Path,
    map: &ClientMap,
) -> Option<String> {
    if let Some(c) = explicit.filter(|c| !c.is_empty()) {
        return Some(c.to_string());
    }
    if let Ok(s) = fs::read_to_string(project_dir.join(".ebouncer").join("client")) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    first_remote_url(project_dir).and_then(|r| map.lookup(&r))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_ssh_and_https_equal() {
        assert_eq!(
            normalize_remote("git@github.com:acme/billing.git"),
            normalize_remote("https://github.com/acme/billing.git")
        );
    }
    #[test]
    fn longest_prefix_wins() {
        let mut m = ClientMap::default();
        m.rules.insert("github.com/acme".into(), "ACME 公司".into());
        m.rules
            .insert("github.com/acme/billing".into(), "ACME 计费项目".into());
        assert_eq!(
            m.lookup("github.com/acme/billing").unwrap(),
            "ACME 计费项目"
        );
        assert_eq!(m.lookup("github.com/acme/other").unwrap(), "ACME 公司");
    }
}
