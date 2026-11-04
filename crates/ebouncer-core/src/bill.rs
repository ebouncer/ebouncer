// SPDX-License-Identifier: AGPL-3.0-only
//! 客户账单（ebc bill）：按客户归因的费用分项单。
//! 结构对齐 schemas/invoice.schema.json。计价复用 rates::price_at（按事件时间选生效版本与
//! 高峰/空闲价档），每个价格带来源（vendor + effective_from + 价档），金额保留估算口径
//! 说明——诚实是开票功能的生命线。

use crate::ledger::LedgerEvent;
use crate::rates::{Tier, VendorRates, price_at, tool_entry_at};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Period {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillItem {
    pub tool: String,
    pub category: String, // token_input | token_output | cache_read | search_request | fetch_request
    pub qty: u64,
    pub unit_price_per_k_or_mtok: f64,
    pub amount: f64,
    pub price_source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub client: String,
    pub period: Period,
    pub items: Vec<BillItem>,
    pub markup: f64,
    pub total: f64,
    pub currency: String,
    pub generated_at: String,
    pub data_basis: Option<String>,
    /// 本次账单实际命中的费率条目原文（按厂商+生效版本去重排序）。
    /// 账单因此自带价格依据：日后费率表被改，也能回答"这张单当时按哪一版、哪些单价算的"。
    pub rates_snapshot: Vec<VendorRates>,
    /// 快照的 sha256；同一账本 + 同一费率表重开账单应得到同一指纹（可复现性校验）。
    pub rates_fingerprint: String,
    pub watermark: bool,
}

/// 账单分项的计价锚点：同一 tool×model 可能跨多个费率版本、也可能跨高峰/空闲价档，
/// 二者任一不同都必须拆行，否则整单会被单一单价"锁死"（2026-11-04 修复）。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct RateKey {
    vendor: String,
    effective_from: String,
    tier: Tier,
}

/// 一组事件的单价与来源文案（分组时确定，组内恒定）。
#[derive(Clone, Debug)]
struct UnitPrices {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    search: Option<f64>,
    fetch: Option<f64>,
    source: String,
}

#[derive(Default)]
struct Qty {
    tokens_in: u64,
    tokens_out: u64,
    cache_read: u64,
    cache_write: u64,
    requests_search: u64,
    requests_fetch: u64,
}

/// 账单构建失败的原因。v0.1 只支持单币种账单：同一客户跨币种（如 CNY + USD）
/// 时明确拒绝，不做隐式汇率换算（汇率需要可追溯快照，留给后续版本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillError {
    MixedCurrency(Vec<String>),
}

impl std::fmt::Display for BillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BillError::MixedCurrency(list) => write!(
                f,
                "该客户的事件跨币种（{}），v0.1 只支持单币种账单；请先在费率表统一币种或按币种拆客户",
                list.join(" / ")
            ),
        }
    }
}

impl std::error::Error for BillError {}

/// 解析一条事件的计价锚点与单价（同时返回命中的费率条目，供账单冻结快照）：
/// - 有模型：走 rates::price_at（版本选择 + 高峰/空闲价档）；
/// - 无模型（纯 Search/Fetch 按次事件）：走 rates::tool_entry_at 取按次价，价档记为统一价
///   （官方按次价无时段折扣，与 rates::estimate 的口径一致）。
///
/// 修复（2026-11-04）：此前无模型事件在账单里被当成"无价模型"记 0 元，而 `ebc ledger`
/// 却按按次价计入，同一份账本两处金额不一致。
fn resolve_price<'a>(
    model: &str,
    ev_ts: &str,
    table: &'a [VendorRates],
) -> Option<(&'a VendorRates, RateKey, UnitPrices)> {
    if model == "-" {
        let entry = tool_entry_at(ev_ts, table)?;
        let tr = entry.tool_requests.as_ref()?;
        return Some((
            entry,
            RateKey {
                vendor: entry.vendor.clone(),
                effective_from: entry.effective_from.clone(),
                tier: Tier::Flat,
            },
            UnitPrices {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                search: Some(tr.search_per_k),
                fetch: Some(tr.fetch_per_k),
                source: format!(
                    "费率表 {}@{} · 按次价（无时段折扣）",
                    entry.vendor, entry.effective_from
                ),
            },
        ));
    }
    let p = price_at(model, ev_ts, table)?;
    Some((
        p.entry,
        RateKey {
            vendor: p.entry.vendor.clone(),
            effective_from: p.entry.effective_from.clone(),
            tier: p.tier,
        },
        UnitPrices {
            input: p.input_per_mtok(),
            output: p.output_per_mtok(),
            cache_read: p.cache_read_per_mtok(),
            cache_write: p.cache_write_per_mtok(),
            search: p.tool_requests.map(|t| t.search_per_k),
            fetch: p.tool_requests.map(|t| t.fetch_per_k),
            source: p.source(),
        },
    ))
}

/// 从账本事件构建某客户的账单。数量按 (tool, model, 生效费率条目) 聚合：
/// 版本选择发生在**每条事件**上，同一模型跨调价窗口会拆成多行、各带自己的单价与
/// price_source——账单金额因此与 `ebc ledger` 的汇总口径一致。
pub fn build(
    client: &str,
    events: &[LedgerEvent],
    table: &[VendorRates],
    markup: f64,
    watermark: bool,
    data_basis: Option<String>,
) -> Result<Invoice, BillError> {
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

    // 聚合：(tool, model, 费率条目+价档) → 数量 + 单价
    let mut agg: std::collections::BTreeMap<
        (String, String, Option<RateKey>),
        (Qty, Option<UnitPrices>),
    > = Default::default();
    // 价格快照：只冻结本单实际命中的条目（vendor+生效版本去重，BTreeMap 保证顺序稳定）
    let mut used_entries: std::collections::BTreeMap<(String, String), VendorRates> =
        Default::default();
    // 单币种约束：只由"命中了费率条目"的事件决定；无价模型不参与（金额本来就是 0）
    let mut currencies: std::collections::BTreeSet<String> = Default::default();
    for e in &evts {
        let model = e.model.clone().unwrap_or_else(|| "-".to_string());
        let resolved = resolve_price(&model, &e.ts, table);
        if let Some((entry, _, _)) = &resolved {
            used_entries
                .entry((entry.vendor.clone(), entry.effective_from.clone()))
                .or_insert_with(|| (*entry).clone());
            currencies.insert(entry.currency.clone());
        }
        let key = resolved.as_ref().map(|(_, k, _)| k.clone());
        let units = resolved.as_ref().map(|(_, _, u)| u.clone());
        let entry = agg
            .entry((e.tool.clone(), model, key))
            .or_insert_with(|| (Qty::default(), units));
        entry.0.tokens_in += e.tokens_in;
        entry.0.tokens_out += e.tokens_out;
        entry.0.cache_read += e.cache_read;
        entry.0.cache_write += e.cache_write;
        entry.0.requests_search += e.requests_search;
        entry.0.requests_fetch += e.requests_fetch;
    }
    if currencies.len() > 1 {
        return Err(BillError::MixedCurrency(currencies.into_iter().collect()));
    }
    let currency = currencies
        .into_iter()
        .next()
        .or_else(|| evts.first().map(|e| e.currency.clone()))
        .unwrap_or_else(|| "CNY".to_string());
    let rates_snapshot: Vec<VendorRates> = used_entries.into_values().collect();
    let rates_fingerprint = crate::rates::snapshot_fingerprint(&rates_snapshot);

    let mut items: Vec<BillItem> = Vec::new();
    let mut raw_total = 0.0f64;
    for ((tool, model, _), (q, units)) in &agg {
        match units {
            Some(u) => {
                let mut push = |category: &str, qty: u64, unit: f64, div: f64| {
                    if qty == 0 {
                        return;
                    }
                    let amount = qty as f64 * unit / div;
                    raw_total += amount;
                    // 记了量但该条目没单独定价（如 DeepSeek 的写缓存已含在未命中输入价里）：
                    // 保留 0 元行并写明原因，避免被误读成"漏算"。
                    let note = (category == "cache_write" && unit == 0.0)
                        .then(|| "该费率条目未单独收取缓存写入费用（默认 0，写入已含在缓存未命中输入价内）".to_string());
                    items.push(BillItem {
                        tool: tool.clone(),
                        category: category.to_string(),
                        qty,
                        unit_price_per_k_or_mtok: unit,
                        amount,
                        price_source: u.source.clone(),
                        note,
                    });
                };
                push("token_input", q.tokens_in, u.input, 1e6);
                push("token_output", q.tokens_out, u.output, 1e6);
                push("cache_read", q.cache_read, u.cache_read, 1e6);
                push("cache_write", q.cache_write, u.cache_write, 1e6);
                if let Some(v) = u.search {
                    push("search_request", q.requests_search, v, 1e3);
                }
                if let Some(v) = u.fetch {
                    push("fetch_request", q.requests_fetch, v, 1e3);
                }
            }
            None => {
                // 无价模型：数量入列、金额 0，note 声明——绝不静默丢行，也绝不编价
                for (cat, qty) in [
                    ("token_input", q.tokens_in),
                    ("token_output", q.tokens_out),
                    ("cache_read", q.cache_read),
                    ("cache_write", q.cache_write),
                    ("search_request", q.requests_search),
                    ("fetch_request", q.requests_fetch),
                ] {
                    if qty > 0 {
                        items.push(BillItem {
                            tool: tool.clone(),
                            category: cat.to_string(),
                            qty,
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
    Ok(Invoice {
        client: client.to_string(),
        period: Period { from, to },
        items,
        markup,
        total,
        currency,
        generated_at: chrono::Utc::now().to_rfc3339(),
        data_basis,
        rates_snapshot,
        rates_fingerprint,
        watermark,
    })
}

impl Invoice {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// CSV：头注释写口径（PRD §3.3.5 红线）；带费率快照指纹，导出件也能回对价格依据。
    pub fn to_csv(&self) -> String {
        let mut s = format!(
            "# 客户: {} · 周期: {} ~ {} · 口径: token×单价+按次 · 加成 {:.0}% · 估算未含税 · 水印 {}\n",
            self.client,
            self.period.from,
            self.period.to,
            self.markup * 100.0,
            if self.watermark { "是" } else { "否" }
        );
        s.push_str(&format!(
            "# 账本指纹: {} · 费率表指纹: {}（条目快照见 JSON 导出的 rates_snapshot）\n",
            self.data_basis.as_deref().unwrap_or("-"),
            self.rates_fingerprint
        ));
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
    use crate::rates::{ModelRate, TimeTiers, ToolRequestRate, VendorRates};

    fn ev(
        client: Option<&str>,
        model: &str,
        ti: u64,
        to: u64,
        cr: u64,
        rs: u64,
        rf: u64,
    ) -> LedgerEvent {
        LedgerEvent {
            ts: "2026-10-15T00:00:00Z".into(),
            session_id: "s".into(),
            tool: "dsh".into(),
            project: "billing".into(),
            client: client.map(str::to_string),
            model: Some(model.into()),
            tokens_in: ti,
            tokens_out: to,
            cache_read: cr,
            cache_write: 0,
            cost_native: 0.0,
            currency: "CNY".into(),
            requests_search: rs,
            requests_fetch: rf,
        }
    }
    fn table() -> Vec<VendorRates> {
        vec![VendorRates {
            vendor: "deepseek".into(),
            currency: "CNY".into(),
            effective_from: "2026-09-01".into(),
            time_tiers: None,
            models: vec![ModelRate {
                name: "deepseek-flash".into(),
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
                cache_read_per_mtok: 0.1,
                cache_write_per_mtok: 0.0,
            }],
            tool_requests: Some(ToolRequestRate {
                search_per_k: 5.0,
                fetch_per_k: 5.0,
            }),
        }]
    }
    #[test]
    fn builds_invoice_with_markup() {
        let evs = vec![
            ev(
                Some("ACME"),
                "deepseek-flash",
                1_000_000,
                1_000_000,
                1_000_000,
                1000,
                1000,
            ),
            ev(None, "deepseek-flash", 999_999_999, 0, 0, 0, 0), // 别的客户不应混入
        ];
        let inv = build("ACME", &evs, &table(), 0.2, true, None).unwrap();
        assert_eq!(inv.items.len(), 5);
        // 1.0 + 2.0 + 0.1 + 5.0 + 5.0 = 13.1，×1.2 = 15.72
        assert!((inv.total - 15.72).abs() < 1e-6, "total={}", inv.total);
        assert!(inv.watermark);
    }
    #[test]
    fn unpriced_model_lists_zero_amount_without_inventing_price() {
        let evs = vec![ev(Some("X"), "mystery-model", 5000, 100, 0, 0, 0)];
        let inv = build("X", &evs, &table(), 0.0, true, None).unwrap();
        assert!(inv.items.iter().all(|i| i.amount == 0.0));
        assert!(inv.items.iter().any(|i| i.note.as_deref().is_some()));
    }
    #[test]
    fn csv_has_basis_header() {
        let evs = vec![ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0)];
        let csv = build("ACME", &evs, &table(), 0.0, true, None)
            .unwrap()
            .to_csv();
        assert!(csv.starts_with("# 客户: ACME"));
        assert!(csv.contains("口径:"));
        assert!(csv.contains("费率表指纹:"), "{csv}");
    }

    /// 回归（2026-11-04）：cache_write 必须单列成行并随价档打折
    ///（此前只记量、账单里金额恒为 0，Anthropic 语义下会系统性少算）。
    #[test]
    fn bills_cache_write_with_tier_multiplier() {
        let table = vec![VendorRates {
            vendor: "deepseek".into(),
            currency: "CNY".into(),
            effective_from: "2026-11-01".into(),
            time_tiers: Some(TimeTiers {
                peak_windows: vec!["09:00-12:00".into()],
                utc_offset_minutes: 480,
                weekdays_only: true,
                holiday_dates: vec![],
                off_peak_multiplier: 0.5,
            }),
            models: vec![ModelRate {
                name: "cached-model".into(),
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.3,
                cache_write_per_mtok: 3.75,
            }],
            tool_requests: None,
        }];
        let mut peak = ev(Some("ACME"), "cached-model", 0, 0, 0, 0, 0);
        peak.ts = "2026-11-03T02:00:00Z".into(); // 周二 10:00 北京 → 高峰
        peak.cache_write = 1_000_000;
        let mut idle = ev(Some("ACME"), "cached-model", 0, 0, 0, 0, 0);
        idle.ts = "2026-11-03T15:00:00Z".into(); // 周二 23:00 北京 → 空闲半价
        idle.cache_write = 1_000_000;
        let inv = build("ACME", &[peak, idle], &table, 0.0, true, None).unwrap();
        assert_eq!(inv.items.len(), 2, "{:?}", inv.items);
        assert!(inv.items.iter().all(|i| i.category == "cache_write"));
        assert!(
            (inv.total - (3.75 + 1.875)).abs() < 1e-9,
            "total={}",
            inv.total
        );
        assert!(inv.items.iter().any(|i| i.price_source.contains("高峰")));
        assert!(inv.items.iter().any(|i| i.price_source.contains("空闲")));
    }

    /// 未配置 cache_write 单价时仍保留 0 元行并注明原因，不静默丢量。
    #[test]
    fn zero_priced_cache_write_row_is_explained() {
        let mut e = ev(Some("ACME"), "deepseek-flash", 0, 0, 0, 0, 0);
        e.cache_write = 500_000;
        let inv = build("ACME", &[e], &table(), 0.0, true, None).unwrap();
        let row = inv
            .items
            .iter()
            .find(|i| i.category == "cache_write")
            .expect("应保留 cache_write 行");
        assert_eq!(row.amount, 0.0);
        assert!(
            row.note.as_deref().unwrap_or("").contains("未单独收取"),
            "{:?}",
            row.note
        );
    }

    /// 账单自带价格依据：只冻结本单命中的费率条目，表内顺序不影响指纹，
    /// 而用到的价格一变指纹必须变（复现性校验）。
    #[test]
    fn freezes_used_rate_entries_with_stable_fingerprint() {
        let v1 = VendorRates {
            vendor: "deepseek".into(),
            currency: "CNY".into(),
            effective_from: "2026-09-01".into(),
            time_tiers: None,
            models: vec![ModelRate {
                name: "deepseek-flash".into(),
                input_per_mtok: 1.0,
                output_per_mtok: 0.0,
                cache_read_per_mtok: 0.0,
                cache_write_per_mtok: 0.0,
            }],
            tool_requests: None,
        };
        let v2 = VendorRates {
            vendor: "deepseek".into(),
            currency: "CNY".into(),
            effective_from: "2026-11-01".into(),
            time_tiers: None,
            models: vec![ModelRate {
                name: "deepseek-flash".into(),
                input_per_mtok: 2.0,
                output_per_mtok: 0.0,
                cache_read_per_mtok: 0.0,
                cache_write_per_mtok: 0.0,
            }],
            tool_requests: None,
        };
        let mut e = ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0);
        e.ts = "2026-11-03T00:00:00Z".into();

        let inv = build(
            "ACME",
            std::slice::from_ref(&e),
            &[v1.clone(), v2.clone()],
            0.0,
            true,
            None,
        )
        .unwrap();
        assert_eq!(inv.rates_snapshot.len(), 1, "只应冻结命中的版本");
        assert_eq!(inv.rates_snapshot[0].effective_from, "2026-11-01");
        assert_eq!(inv.rates_fingerprint.len(), 64);
        assert!(inv.rates_fingerprint.chars().all(|c| c.is_ascii_hexdigit()));

        // 费率表顺序变化不影响快照指纹（可复现）
        let rev = build(
            "ACME",
            std::slice::from_ref(&e),
            &[v2.clone(), v1.clone()],
            0.0,
            true,
            None,
        )
        .unwrap();
        assert_eq!(rev.rates_fingerprint, inv.rates_fingerprint);

        // 用到的单价变化 → 指纹必须变化
        let mut v2b = v2;
        v2b.models[0].input_per_mtok = 3.0;
        let changed = build(
            "ACME",
            std::slice::from_ref(&e),
            &[v1, v2b],
            0.0,
            true,
            None,
        )
        .unwrap();
        assert_ne!(changed.rates_fingerprint, inv.rates_fingerprint);
    }

    /// 回归（2026-11-04）：纯 Search/Fetch 事件（无模型）必须按费率表的按次价计费，
    /// 不能记 0 元——否则 `ebc bill` 与 `ebc ledger` 对同一账本给出不同金额。
    #[test]
    fn prices_tool_only_events_from_per_request_rates() {
        let mut e = ev(Some("ACME"), "-", 0, 0, 0, 1000, 500);
        e.model = None;
        let evs = vec![e];
        let t = table();
        let inv = build("ACME", &evs, &t, 0.0, true, None).unwrap();
        assert_eq!(inv.items.len(), 2, "{:?}", inv.items);
        assert!((inv.total - 7.5).abs() < 1e-9, "total={}", inv.total);
        assert!(inv.items.iter().all(|i| i.amount > 0.0));
        assert!(
            inv.items.iter().any(|i| i.price_source.contains("按次价")),
            "{:?}",
            inv.items
        );
        // 与账本汇总（rates::estimate）口径一致
        let estimated: f64 = evs.iter().map(|x| crate::rates::estimate(x, &t)).sum();
        assert!(
            (inv.total - estimated).abs() < 1e-9,
            "bill={} ledger={}",
            inv.total,
            estimated
        );
    }

    /// 回归（2026-11-04）：同一客户跨调价窗口时，账单必须按费率版本拆行，
    /// 而不是整单用窗口内单一时间点的单价（此前会按最早事件时间锁价）。
    #[test]
    fn splits_items_by_rate_version_across_price_change() {
        let table = vec![
            VendorRates {
                vendor: "deepseek".into(),
                currency: "CNY".into(),
                effective_from: "2026-09-01".into(),
                time_tiers: None,
                models: vec![ModelRate {
                    name: "deepseek-flash".into(),
                    input_per_mtok: 1.0,
                    output_per_mtok: 0.0,
                    cache_read_per_mtok: 0.0,
                    cache_write_per_mtok: 0.0,
                }],
                tool_requests: None,
            },
            VendorRates {
                vendor: "deepseek".into(),
                currency: "CNY".into(),
                effective_from: "2026-11-01".into(),
                time_tiers: None,
                models: vec![ModelRate {
                    name: "deepseek-flash".into(),
                    input_per_mtok: 2.0,
                    output_per_mtok: 0.0,
                    cache_read_per_mtok: 0.0,
                    cache_write_per_mtok: 0.0,
                }],
                tool_requests: None,
            },
        ];
        let mut old = ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0);
        old.ts = "2026-10-15T00:00:00Z".into();
        let mut new = ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0);
        new.ts = "2026-11-03T00:00:00Z".into();
        let inv = build("ACME", &[old, new], &table, 0.0, true, None).unwrap();
        assert_eq!(inv.items.len(), 2, "跨版本应拆成两行");
        assert!((inv.total - 3.0).abs() < 1e-9, "total={}", inv.total);
        let sources: Vec<&str> = inv.items.iter().map(|i| i.price_source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("2026-09-01")),
            "{sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("2026-11-01")),
            "{sources:?}"
        );
    }

    /// 回归（2026-11-04）：同一费率版本内跨高峰/空闲两个价档时，账单同样要拆行，
    /// 且两行单价不同（高峰 2.0 / 空闲 1.0）。
    #[test]
    fn splits_items_by_peak_and_off_peak_tier() {
        let table = vec![VendorRates {
            vendor: "deepseek".into(),
            currency: "CNY".into(),
            effective_from: "2026-11-01".into(),
            time_tiers: Some(TimeTiers {
                peak_windows: vec!["09:00-12:00".into(), "14:00-18:00".into()],
                utc_offset_minutes: 480,
                weekdays_only: true,
                holiday_dates: vec![],
                off_peak_multiplier: 0.5,
            }),
            models: vec![ModelRate {
                name: "deepseek-flash".into(),
                input_per_mtok: 2.0,
                output_per_mtok: 0.0,
                cache_read_per_mtok: 0.0,
                cache_write_per_mtok: 0.0,
            }],
            tool_requests: None,
        }];
        let mut peak = ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0);
        peak.ts = "2026-11-03T02:00:00Z".into(); // 周二 10:00 北京 → 高峰
        let mut idle = ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0);
        idle.ts = "2026-11-03T15:00:00Z".into(); // 周二 23:00 北京 → 空闲
        let inv = build("ACME", &[peak, idle], &table, 0.0, true, None).unwrap();
        assert_eq!(inv.items.len(), 2, "高峰/空闲应各成一行");
        assert!(
            (inv.total - 3.0).abs() < 1e-9,
            "2.0 + 1.0 = 3.0，total={}",
            inv.total
        );
        assert!(inv.items.iter().any(|i| i.price_source.contains("高峰")));
        assert!(inv.items.iter().any(|i| i.price_source.contains("空闲")));
    }

    fn usd_table() -> Vec<VendorRates> {
        vec![VendorRates {
            vendor: "anthropic".into(),
            currency: "USD".into(),
            effective_from: "2026-11-01".into(),
            time_tiers: None,
            models: vec![ModelRate {
                name: "claude-sonnet-5-5".into(),
                input_per_mtok: 2.0,
                output_per_mtok: 10.0,
                cache_read_per_mtok: 0.1,
                cache_write_per_mtok: 2.5,
            }],
            tool_requests: None,
        }]
    }

    /// 账单币种跟随命中的费率条目（Anthropic = USD），不再硬编码 CNY。
    #[test]
    fn usd_invoice_uses_entry_currency() {
        let evs = vec![ev(
            Some("ACME"),
            "claude-sonnet-5-5-20260101",
            1_000_000,
            0,
            0,
            0,
            0,
        )];
        let inv = build("ACME", &evs, &usd_table(), 0.0, true, None).unwrap();
        assert_eq!(inv.currency, "USD");
        assert!((inv.total - 2.0).abs() < 1e-9, "total={}", inv.total);
    }

    /// 单币种约束：同一客户跨币种时明确报错，不做隐式汇率换算。
    #[test]
    fn mixed_currency_invoice_is_rejected() {
        let mut cny = ev(Some("ACME"), "deepseek-flash", 1_000_000, 0, 0, 0, 0);
        cny.ts = "2026-11-03T00:00:00Z".into();
        let mut usd = ev(Some("ACME"), "claude-sonnet-5-5", 1_000_000, 0, 0, 0, 0);
        usd.ts = "2026-11-03T00:00:00Z".into();
        usd.currency = "USD".into();
        let mut table = table();
        table.extend(usd_table());
        let err = build("ACME", &[cny, usd], &table, 0.0, true, None).unwrap_err();
        match err {
            BillError::MixedCurrency(list) => {
                assert!(list.contains(&"CNY".to_string()));
                assert!(list.contains(&"USD".to_string()));
            }
        }
    }
}
