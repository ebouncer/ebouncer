// SPDX-License-Identifier: AGPL-3.0-only
//! 费率表：厂商 × 模型 × (输入/输出/缓存) + 工具按次单价（如 DeepSeek Search/Fetch）。
//! 表带 effective_from——费率会变，账本要能按事件发生时点的价格重算。
//! ⚠️ 本文件内置示例价格均为占位，正式数据须以各厂商定价页复核后填入（禁止凭记忆填价）。

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
    pub effective_from: String,
    #[serde(default)]
    pub models: Vec<ModelRate>,
    #[serde(default)]
    pub tool_requests: Option<ToolRequestRate>,
}

pub fn load_dir(dir: &Path) -> Vec<VendorRates> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("yaml") {
                if let Ok(s) = fs::read_to_string(&p) {
                    if let Ok(v) = serde_yaml::from_str::<VendorRates>(&s) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
}

/// 按事件估算成本；找不到模型单价时只计工具按次部分。
pub fn estimate(ev: &LedgerEvent, table: &[VendorRates]) -> f64 {
    let mut cost = 0.0;
    let Some(m) = &ev.model else {
        return tool_cost(ev, table);
    };
    for v in table {
        for mr in &v.models {
            if m == &mr.name || m.ends_with(&mr.name) {
                cost += ev.tokens_in as f64 * mr.input_per_mtok / 1e6
                    + ev.tokens_out as f64 * mr.output_per_mtok / 1e6
                    + ev.cache_read as f64 * mr.cache_read_per_mtok / 1e6;
                return cost + tool_cost(ev, table);
            }
        }
    }
    tool_cost(ev, table)
}

fn tool_cost(ev: &LedgerEvent, table: &[VendorRates]) -> f64 {
    if ev.requests_search + ev.requests_fetch == 0 {
        return 0.0;
    }
    for v in table {
        if let Some(tr) = &v.tool_requests {
            // 工具按次价与模型同 vendor；v0.1 简化：任一表命中即取（vendor 级归并在 v0.2 精化）
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
            tokens_in: 1_000_000, tokens_out: 1_000_000, cache_read: 0,
            cost_native: 0.0, currency: "CNY".into(),
            requests_search: 1000, requests_fetch: 0,
        }
    }
    #[test]
    fn estimate_model_plus_per_k() {
        let t = VendorRates {
            vendor: "deepseek".into(), currency: "CNY".into(), effective_from: "2026-11-01".into(),
            models: vec![ModelRate { name: "deepseek-chat".into(), input_per_mtok: 2.0, output_per_mtok: 3.0, cache_read_per_mtok: 0.0 }],
            tool_requests: Some(ToolRequestRate { search_per_k: 5.0, fetch_per_k: 5.0 }),
        };
        let c = estimate(&ev(), std::slice::from_ref(&t));
        assert!((c - (2.0 + 3.0 + 5.0)).abs() < 1e-9);
    }
}
