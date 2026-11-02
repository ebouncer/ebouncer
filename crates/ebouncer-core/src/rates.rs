// SPDX-License-Identifier: AGPL-3.0-only
//! 费率表：厂商 × 模型 × (输入/输出/缓存) + 工具按次单价（如 DeepSeek Search/Fetch）。
//! **账本记事实（token 量），价格记观点（费率表）**：同一厂商可存多版本条目，
//! estimate() 按事件 ts 匹配 effective_from ≤ ts 的最新版本；早于全部版本的用最早版本兜底。
//! ⚠️ 价格必须以厂商定价页复核后填入并在 yaml 注释标注核验日期，禁止凭记忆填价。

use crate::ledger::LedgerEvent;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRate {
    pub name: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read_per_mtok: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequestRate {
    /// 每千次 Search 成功请求价格
    pub search_per_k: f64,
    /// 每千次 Fetch 成功请求价格
    pub fetch_per_k: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorRates {
    pub vendor: String,
    pub currency: String,
    /// ISO 日期（YYYY-MM-DD）；同一厂商多个版本共存，按事件时间选择生效版本
    pub effective_from: String,
    #[serde(default)]
    pub models: Vec<ModelRate>,
    #[serde(default)]
    pub tool_requests: Option<ToolRequestRate>,
}

fn model_matches(model: &str, mr: &ModelRate) -> bool {
    model == mr.name || model.ends_with(&mr.name)
}

/// 在包含该模型的厂商条目中，选 effective_from ≤ 事件时间 的最新版本；
/// 若事件早于所有版本，退回最早版本（历史数据用当时的价，保守可读）。
fn best_entry<'a>(table: &'a [VendorRates], model: &str, ev_ts: &str) -> Option<&'a VendorRates> {
    let applicable: Vec<&VendorRates> = table
        .iter()
        .filter(|v| v.models.iter().any(|mr| model_matches(model, mr)))
        .collect();
    if applicable.is_empty() {
        return None;
    }
    applicable
        .iter()
        .copied()
        .filter(|v| v.effective_from.as_str() <= ev_ts)
        .max_by_key(|v| v.effective_from.as_str())
        .or_else(|| applicable.iter().copied().min_by_key(|v| v.effective_from.as_str()))
}

/// 返回 (模型, 事件时间) 的生效价格：模型单价与同一条目内的工具按次单价。
/// estimate() 与 bill 分项计价共用 best_entry 的版本选择语义（effective_from ≤ ts 取最新）。
pub fn price_for<'a>(
    model: &str,
    ev_ts: &str,
    table: &'a [VendorRates],
) -> Option<(&'a ModelRate, Option<&'a ToolRequestRate>)> {
    let v = best_entry(table, model, ev_ts)?;
    let mr = v.models.iter().find(|mr| model_matches(model, mr))?;
    Some((mr, v.tool_requests.as_ref()))
}

pub fn load_dir(dir: &Path) -> Vec<VendorRates> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("yaml") {
                if let Ok(s) = fs::read_to_string(&p) {
                    // 一个文件可含多版本条目（yaml 序列或单条目均可）
                    if let Ok(vs) = serde_yaml::from_str::<Vec<VendorRates>>(&s) {
                        out.extend(vs);
                        continue;
                    }
                    if let Ok(v) = serde_yaml::from_str::<VendorRates>(&s) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
}

/// 按事件估算成本：先按时间锁定生效版本，再算 token 价 + 该版本内的工具按次价。
/// 找不到模型单价时只计工具按次部分（沿用旧厂商级兜底）。
pub fn estimate(ev: &LedgerEvent, table: &[VendorRates]) -> f64 {
    let Some(m) = &ev.model else {
        return tool_cost(ev, table);
    };
    let Some(v) = best_entry(table, m, &ev.ts) else {
        return tool_cost(ev, table);
    };
    let mut cost = 0.0;
    for mr in &v.models {
        if model_matches(m, mr) {
            cost += ev.tokens_in as f64 * mr.input_per_mtok / 1e6
                + ev.tokens_out as f64 * mr.output_per_mtok / 1e6
                + ev.cache_read as f64 * mr.cache_read_per_mtok / 1e6;
            break;
        }
    }
    if let Some(tr) = &v.tool_requests {
        cost += ev.requests_search as f64 * tr.search_per_k / 1000.0
            + ev.requests_fetch as f64 * tr.fetch_per_k / 1000.0;
    } else {
        cost += tool_cost(ev, table);
    }
    cost
}

/// 无模型信息的事件兜底：任一含按次价的厂商条目（v0.1 简化）。
fn tool_cost(ev: &LedgerEvent, table: &[VendorRates]) -> f64 {
    if ev.requests_search + ev.requests_fetch == 0 {
        return 0.0;
    }
    for v in table {
        if let Some(tr) = &v.tool_requests {
            return ev.requests_search as f64 * tr.search_per_k / 1000.0
                + ev.requests_fetch as f64 * tr.fetch_per_k / 1000.0;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev() -> LedgerEvent {
        LedgerEvent {
            ts: "2026-11-01T00:00:00Z".into(), session_id: "s".into(), tool: "codex".into(),
            project: "p".into(), client: None, model: Some("deepseek-chat".into()),
            tokens_in: 1_000_000, tokens_out: 1_000_000, cache_read: 0, cache_write: 0,
            cost_native: 0.0, currency: "CNY".into(),
            requests_search: 1000, requests_fetch: 0,
        }
    }
    fn entry(eff: &str, input: f64, output: f64, per_k: f64) -> VendorRates {
        VendorRates {
            vendor: "deepseek".into(), currency: "CNY".into(), effective_from: eff.into(),
            models: vec![ModelRate { name: "deepseek-chat".into(), input_per_mtok: input, output_per_mtok: output, cache_read_per_mtok: 0.0 }],
            tool_requests: Some(ToolRequestRate { search_per_k: per_k, fetch_per_k: per_k }),
        }
    }
    #[test]
    fn estimate_model_plus_per_k() {
        let t = entry("2026-11-01", 2.0, 3.0, 5.0);
        let c = estimate(&ev(), std::slice::from_ref(&t));
        assert!((c - (2.0 + 3.0 + 5.0)).abs() < 1e-9);
    }
    #[test]
    fn picks_version_by_event_time() {
        // 同一厂商两版本：2026-09-01 涨价前 / 2026-11-01 涨价后
        let table = vec![
            entry("2026-09-01", 1.0, 2.0, 5.0),
            entry("2026-11-01", 2.0, 3.0, 8.0),
        ];
        // 事件在旧价期内 → 用旧价（含顺序无关性：旧版本放后面也要选对）
        let mut old = ev();
        old.ts = "2026-10-15T08:00:00Z".into();
        assert!((estimate(&old, &table) - (1.0 + 2.0 + 5.0)).abs() < 1e-9);
        // 事件在新价期内 → 用新价
        let mut new = ev();
        new.ts = "2026-11-05T08:00:00Z".into();
        assert!((estimate(&new, &table) - (2.0 + 3.0 + 8.0)).abs() < 1e-9);
    }
    #[test]
    fn event_older_than_all_versions_uses_oldest() {
        let table = vec![entry("2026-11-01", 2.0, 3.0, 5.0), entry("2026-12-01", 4.0, 6.0, 9.0)];
        let mut e = ev();
        e.ts = "2026-01-01T00:00:00Z".into();
        assert!((estimate(&e, &table) - (2.0 + 3.0 + 5.0)).abs() < 1e-9);
    }
    #[test]
    fn price_for_returns_matching_entry_and_none_when_absent() {
        let table = vec![entry("2026-09-01", 1.0, 2.0, 5.0), entry("2026-11-01", 2.0, 3.0, 8.0)];
        let (mr, tr) = price_for("deepseek-chat", "2026-10-15T00:00:00Z", &table).unwrap();
        assert!((mr.input_per_mtok - 1.0).abs() < 1e-9);
        assert!((tr.unwrap().search_per_k - 5.0).abs() < 1e-9);
        assert!(price_for("no-such-model", "2026-10-15T00:00:00Z", &table).is_none());
    }
}
