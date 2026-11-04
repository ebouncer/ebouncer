// SPDX-License-Identifier: AGPL-3.0-only
//! 计费模式配置：provider → subscription / payg / unknown。
//!
//! 只存识别结果与来源，**不存任何 API Key**。Key 只在 CLI 内存中短暂使用，
//! 由 `ebc plan detect` 调用厂商接口后丢弃。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MODE_SUBSCRIPTION: &str = "subscription";
pub const MODE_PAYG: &str = "payg";
pub const MODE_UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Plans {
    #[serde(default)]
    pub providers: BTreeMap<String, PlanEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanEntry {
    /// subscription | payg | unknown
    #[serde(default)]
    pub mode: String,
    /// 订阅档位：go / explore / build
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// 月费（订阅模式）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// 生效月份，YYYY-MM
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// auto | manual | invoice
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    /// 第三方/自定义 base URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl PlanEntry {
    pub fn new(mode: &str, source: &str) -> Self {
        Self {
            mode: mode.to_string(),
            source: source.to_string(),
            ..Default::default()
        }
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join("plans.yaml")
}

pub fn load(dir: &Path) -> Plans {
    let p = path(dir);
    let Ok(s) = std::fs::read_to_string(&p) else {
        return Plans::default();
    };
    serde_yaml::from_str(&s).unwrap_or_default()
}

pub fn save(dir: &Path, plans: &Plans) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let p = path(dir);
    let tmp = p.with_extension("yaml.tmp");
    let body = serde_yaml::to_string(plans).unwrap_or_else(|_| "providers: {}".into());
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &p)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Minimax,
    Deepseek,
    Openrouter,
    Anthropic,
    Openai,
    Custom,
}

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Provider::Minimax => "minimax",
            Provider::Deepseek => "deepseek",
            Provider::Openrouter => "openrouter",
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Custom => "custom",
        }
    }

    pub fn from_model(model: &str) -> Option<Self> {
        let m = model.to_ascii_lowercase();
        if m.contains("minimax") {
            Some(Provider::Minimax)
        } else if m.contains("deepseek") {
            Some(Provider::Deepseek)
        } else if m.contains("openrouter") {
            Some(Provider::Openrouter)
        } else if m.contains("claude") {
            Some(Provider::Anthropic)
        } else if m.contains("gpt") || m.starts_with("o1") || m.starts_with("o3") {
            Some(Provider::Openai)
        } else {
            None
        }
    }

    pub fn from_base_url(url: &str) -> Option<Self> {
        let u = url.to_ascii_lowercase();
        if u.contains("minimax") {
            Some(Provider::Minimax)
        } else if u.contains("deepseek") {
            Some(Provider::Deepseek)
        } else if u.contains("openrouter") {
            Some(Provider::Openrouter)
        } else if u.contains("anthropic") {
            Some(Provider::Anthropic)
        } else if u.contains("openai") {
            Some(Provider::Openai)
        } else if u.starts_with("http://") || u.starts_with("https://") {
            Some(Provider::Custom)
        } else {
            None
        }
    }

    pub fn from_key_prefix(key: &str) -> Option<Self> {
        if key.starts_with("sk-or-") {
            Some(Provider::Openrouter)
        } else if key.starts_with("sk-ant-") {
            Some(Provider::Anthropic)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_from_model_and_url() {
        assert_eq!(
            Provider::from_model("claude-sonnet-4-5"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::from_model("MiniMax-M3.1-Flash"),
            Some(Provider::Minimax)
        );
        assert_eq!(
            Provider::from_model("deepseek-flash"),
            Some(Provider::Deepseek)
        );
        assert_eq!(
            Provider::from_key_prefix("sk-or-v1-x"),
            Some(Provider::Openrouter)
        );
        assert_eq!(
            Provider::from_base_url("https://api.deepseek.com"),
            Some(Provider::Deepseek)
        );
        assert_eq!(Provider::from_model("mimo"), None);
    }

    #[test]
    fn plans_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "ebc-plan-{}",
            std::process::id() as u64 + chrono::Utc::now().timestamp_subsec_nanos() as u64
        ));
        let mut plans = Plans::default();
        plans
            .providers
            .insert("minimax".into(), PlanEntry::new(MODE_SUBSCRIPTION, "auto"));
        save(&dir, &plans).unwrap();
        let got = load(&dir);
        assert_eq!(got.providers["minimax"].mode, MODE_SUBSCRIPTION);
        std::fs::remove_dir_all(&dir).ok();
    }
}
