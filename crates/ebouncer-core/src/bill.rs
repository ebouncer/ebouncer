// SPDX-License-Identifier: AGPL-3.0-only
//! 客户账单（ebc bill）：按客户归因的费用分项单。
//! 结构对齐 schemas/invoice.schema.json。计价复用 rates::price_for（按事件时间选生效版本），
//! 每个价格带来源（vendor + effective_from），金额保留估算口径说明——诚实是开票功能的生命线。

use crate::ledger::LedgerEvent;
use crate::rates::{price_for, VendorRates};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Period {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BillItem {
    pub tool: String,
    pub category: String, // token_input | token_output | cache_read | search_request | fetch_request
    pub qty: u64,
    pub unit_price_per_k_or_mtok: f64,
    pub amount: f64,
    pub price_source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Invoice {
    pub client: String,
    pub period: Period,
    pub items: Vec<BillItem>,
    pub markup: f64,
    pub total: f64,
    pub currency: String,
    pub generated_at: String,
    pub data_basis: Option<String>,
    pub watermark: bool,
}

/// 从账本事件构建某客户的账单。qty 按类别聚合；价格取聚合窗口内任一事件时间的生效价
/// （同厂商调价跨窗口的极端情况 v0.1 不处理，出现时按窗口首事件时间计价并在 note 声明）。
pub fn build(
    client: &str,
    events: &[LedgerEvent],
    table: &[VendorRates],
    markup: f64,
    watermark: bool,
    data_basis: Option<String>,
) -> Invoice {
    let evts: Vec<&LedgerEvent> = events
        .iter()
        .filter(|e| e.client.as_deref() == Some(client))
        .collect();
    let from = evts
        .iter()
        .map(|e| e.ts.as_str())
        .min()
        .unwrap_or("-")
        .to_string();
    let to = evts
        .iter()
        .map(|e| e.ts.as_str())
        .max()
        .unwrap_or("-")
        .to_string();

    // 聚合：(tool, model) → 各类数量
    let mut agg: std::collections::BTreeMap<(String, String), (u64, u64, u64, u64, u64)> =
        Default::default();
    for e in &evts {
        let k = (
            e.tool.clone(),
            e.model.clone().unwrap_or_else(|| "-".to_string()),
        );
        let a = agg.entry(k).or_default();
        a.0 += e.tokens_in;
        a.1 += e.tokens_out;
        a.2 += e.cache_read;
        a.3 += e.requests_search;
        a.4 += e.requests_fetch;
    }

    let mut items: Vec<BillItem> = Vec::new();
    let mut raw_total = 0.0f64;
    let ref_ts = if from == "-" { "" } else { from.as_str() };
    for ((tool, model), (ti, to2, cr, rs, rf)) in &agg {
        let pr = if model == "-" {
            None
        } else {
            price_for(model, ref_ts, table)
        };
        let mut push = |category: &str, qty: u64, unit: f64, div: f64| {
            if qty == 0 {
                return;
            }
            let amount = qty as f64 * unit / div;
            raw_total += amount;
            items.push(BillItem {
                tool: tool.clone(),
                category: category.to_string(),
                qty,
                unit_price_per_k_or_mtok: unit,
                amount,
                price_source: format!(
                    "费率表版本 {}",
                    if ref_ts.is_empty() { "最新" } else { ref_ts }
                ),
                note: None,
            });
        };
        match pr {
            Some((mr, tr)) => {
                push("token_input", *ti, mr.input_per_mtok, 1e6);
                push("token_output", *to2, mr.output_per_mtok, 1e6);
                push("cache_read", *cr, mr.cache_read_per_mtok, 1e6);
                if let Some(tr) = tr {
                    push("search_request", *rs, tr.search_per_k, 1e3);
                    push("fetch_request", *rf, tr.fetch_per_k, 1e3);
                }
            }
            None => {
                // 无价模型：数量入列、金额 0，note 声明——绝不静默丢行，也绝不编价
                for (cat, q) in [
                    ("token_input", ti),
                    ("token_output", to2),
                    ("cache_read", cr),
                    ("search_request", rs),
                    ("fetch_request", rf),
                ] {
                    if *q > 0 {
                        items.push(BillItem {
                            tool: tool.clone(),
                            category: cat.to_string(),
                            qty: *q,
                            unit_price_per_k_or_mtok: 0.0,
                            amount: 0.0,
                            price_source: "无价格条目（费率表待补，禁止估价）".to_string(),
                            note: Some(format!("模型 {model} 未配置单价")),
                        });
                    }
                }
            }
        }
    }

    let total = raw_total * (1.0 + markup.max(0.0));
    Invoice {
        client: client.to_string(),
        period: Period { from, to },
        items,
        markup,
        total,
        currency: "CNY".to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        data_basis,
        watermark,
    }
}

impl Invoice {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// CSV：头注释写口径（PRD §3.3.5 红线）。
    pub fn to_csv(&self) -> String {
        let mut s = format!(
            "# 客户: {} · 周期: {} ~ {} · 口径: token×单价+按次 · 加成 {:.0}% · 估算未含税 · 水印 {}\n",
            self.client,
            self.period.from,
            self.period.to,
            self.markup * 100.0,
            if self.watermark { "是" } else { "否" }
        );
        s.push_str("tool,category,qty,unit_price,amount,currency\n");
        for i in &self.items {
            s.push_str(&format!(
                "{},{},{},{:.4},{:.4},{}\n",
                i.tool, i.category, i.qty, i.unit_price_per_k_or_mtok, i.amount, self.currency
            ));
        }
        s.push_str(&format!(",,TOTAL,,, {:.4}\n", self.total));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rates::{ModelRate, ToolRequestRate, VendorRates};

    fn ev(client: Option<&str>, model: &str, ti: u64, to: u64, cr: u64, rs: u64, rf: u64) -> LedgerEvent {
        LedgerEvent {
            ts: "2026-10-15T00:00:00Z".into(), session_id: "s".into(), tool: "dsh".into(),
            project: "billing".into(), client: client.map(str::to_string), model: Some(model.into()),
            tokens_in: ti, tokens_out: to, cache_read: cr, cache_write: 0, cost_native: 0.0,
            currency: "CNY".into(), requests_search: rs, requests_fetch: rf,
        }
    }
    fn table() -> Vec<VendorRates> {
        vec![VendorRates {
            vendor: "deepseek".into(), currency: "CNY".into(), effective_from: "2026-09-01".into(),
            models: vec![ModelRate { name: "deepseek-flash".into(), input_per_mtok: 1.0, output_per_mtok: 2.0, cache_read_per_mtok: 0.1 }],
            tool_requests: Some(ToolRequestRate { search_per_k: 5.0, fetch_per_k: 5.0 }),
        }]
    }
    #[test]
    fn builds_invoice_with_markup() {
        let evs = vec![
            ev(Some("ACME"), "deepseek-flash", 1_000_000, 1_000_000, 1_000_000, 1000, 1000),
            ev(None, "deepseek-flash", 999_999_999, 0, 0, 0, 0), // 别的客户不应混入
        ];
        let inv = build("ACME", &evs, &table(), 0.2, true, None);
        assert_eq!(inv.items.len(), 5);
        // 1.0 + 2.0 + 0.1 + 5.0 + 5.0 = 13.1，×1.2 = 15.72
        assert!((inv.total - 15.72).abs() < 1e-6, "total={}", inv.total);
        assert!(inv.watermark);
    }
    #[test]
    fn unpriced_model_lists_zero_amount_without_inventing_price() {
        let evs = vec![ev(Some("X"), "mystery-model", 5000, 100, 0, 0, 0)];
        let inv = build("X", &evs, &table(), 0.0, true, None);
        assert!(inv.items.iter().all(|i| i.amount == 0.0));
        assert!(inv.items.iter().any(|i| i.note.as_deref().is_some()));
    }
    #[test]
    fn csv_has_basis_header() {
        let evs = vec![ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0)];
        let csv = build("ACME", &evs, &table(), 0.0, true, None).to_csv();
        assert!(csv.starts_with("# 客户: ACME"));
        assert!(csv.contains("口径:"));
    }
}
