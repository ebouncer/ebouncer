// SPDX-License-Identifier: AGPL-3.0-only
//! 规则引擎：YAML 规则库匹配命令 → 决策。v0.1 默认 dry-run：只记日志不阻断（D1 零摩擦接入）。
//! 行内豁免：命令含 `#ebc:allow` 直接放行（PRD 设计保留）。

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub pattern: String,
    /// low | medium | high | critical
    pub severity: String,
    /// ask | deny（dry-run 下一律降级为 log）
    pub action: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleSet {
    pub version: String,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl RuleSet {
    pub fn load(path: &Path) -> Result<Self, String> {
        let s = fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_yaml::from_str(&s).map_err(|e| e.to_string())
    }

    /// 返回 (decision, matched rule_id)。decision: allow | log | ask | deny
    pub fn evaluate(&self, cmd: &str, dry_run: bool) -> (String, Option<String>) {
        if cmd.contains("#ebc:allow") {
            return ("allow".into(), None);
        }
        for r in &self.rules {
            if let Ok(re) = Regex::new(&r.pattern) {
                if re.is_match(cmd) {
                    let decision = match r.action.as_str() {
                        "deny" => if dry_run { "log-deny" } else { "deny" },
                        "ask" => if dry_run { "log-ask" } else { "ask" },
                        _ => "log",
                    };
                    return (decision.into(), Some(r.id.clone()));
                }
            }
        }
        ("allow".into(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn set() -> RuleSet {
        RuleSet {
            version: "test".into(),
            rules: vec![
                Rule { id: "rm-rf-root".into(), pattern: r"rm\s+(-[a-zA-Z]*[rR][a-zA-Z]*\s+)+(/|~|\$HOME)(\s|$)".into(), severity: "critical".into(), action: "deny".into(), note: String::new() },
                Rule { id: "git-clean".into(), pattern: r"git\s+clean\s+-[a-zA-Z]*f".into(), severity: "high".into(), action: "ask".into(), note: String::new() },
            ],
        }
    }
    #[test]
    fn dryrun_downgrades() {
        let s = set();
        let (d, id) = s.evaluate("rm -rf ~ --no-preserve-root", true);
        assert_eq!(d, "log-deny");
        assert_eq!(id.unwrap(), "rm-rf-root");
        assert_eq!(s.evaluate("git clean -fd", true).0, "log-ask");
        assert_eq!(s.evaluate("ls -la", true).0, "allow");
    }
    #[test]
    fn inline_exemption() {
        let s = set();
        assert_eq!(s.evaluate("rm -rf ~/ #ebc:allow", false).0, "allow");
    }
    #[test]
    fn enforce_mode_denies() {
        let s = set();
        assert_eq!(s.evaluate("rm -rf / ", false).0, "deny");
    }
}
