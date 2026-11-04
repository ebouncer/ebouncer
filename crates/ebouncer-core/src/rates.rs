// SPDX-License-Identifier: AGPL-3.0-only
//! 费率表：厂商 × 模型 × (输入/输出/缓存) + 工具按次单价（如 DeepSeek Search/Fetch）。
//! **账本记事实（token 量），价格记观点（费率表）**：同一厂商可存多版本条目，
//! estimate() 按事件 ts 匹配 effective_from ≤ ts 的最新版本；早于全部版本的用最早版本兜底。
//! 版本之上还有时段维度（高峰/空闲）：time_tiers 用厂商本地时间定义高峰窗口，窗口外
//! 按 off_peak_multiplier 打折——同一版本内的事件因此可能落在不同价档，账单必须拆行。
//! ⚠️ 价格必须以厂商定价页复核后填入并在 yaml 注释标注核验日期，禁止凭记忆填价。

use crate::ledger::LedgerEvent;
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Timelike, Weekday};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
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

/// 时段化计价（厂商本地时间）。缺省时该版本全天统一价，行为与旧费率表兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeTiers {
    /// 高峰窗口（本地 "HH:MM-HH:MM"，可多条；start > end 表示跨午夜）
    pub peak_windows: Vec<String>,
    /// 本地时间相对 UTC 的固定偏移（分钟）。北京时间 = 480；中国无夏令时，固定偏移够用
    #[serde(default)]
    pub utc_offset_minutes: i32,
    /// 高峰期仅限周一至周五；周末全天按空闲价
    #[serde(default)]
    pub weekdays_only: bool,
    /// 法定节假日（"YYYY-MM-DD"）全天按空闲价。节假日无法本地推导，需显式配置
    #[serde(default)]
    pub holiday_dates: Vec<String>,
    /// 空闲价相对高峰价的倍数（0.5 = 半价）
    #[serde(default = "default_multiplier")]
    pub off_peak_multiplier: f64,
}

fn default_multiplier() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorRates {
    pub vendor: String,
    pub currency: String,
    /// ISO 日期（YYYY-MM-DD）或 RFC3339；同一厂商多个版本共存，按事件时间选择生效版本
    pub effective_from: String,
    /// 时段价档（可选）；None = 全天统一价
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_tiers: Option<TimeTiers>,
    #[serde(default)]
    pub models: Vec<ModelRate>,
    #[serde(default)]
    pub tool_requests: Option<ToolRequestRate>,
}

/// 事件落在哪个价档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// 该版本没有时段定义：全天统一价
    Flat,
    Peak,
    OffPeak,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Flat => "统一价",
            Tier::Peak => "高峰价",
            Tier::OffPeak => "空闲价",
        }
    }
}

/// 某一事件的实际单价（已含时段倍数）与来源说明。
/// 账单用它写入 `price_source`，人工可据此回查费率表条目与价档。
#[derive(Debug, Clone, Copy)]
pub struct Price<'a> {
    pub entry: &'a VendorRates,
    pub model: &'a ModelRate,
    pub tool_requests: Option<&'a ToolRequestRate>,
    pub tier: Tier,
    pub multiplier: f64,
}

impl Price<'_> {
    pub fn input_per_mtok(&self) -> f64 {
        self.model.input_per_mtok * self.multiplier
    }

    pub fn output_per_mtok(&self) -> f64 {
        self.model.output_per_mtok * self.multiplier
    }

    pub fn cache_read_per_mtok(&self) -> f64 {
        self.model.cache_read_per_mtok * self.multiplier
    }

    /// 账单 `price_source` 文案：厂商 + 生效版本 + 价档（含折扣倍数，便于复核）
    pub fn source(&self) -> String {
        let base = format!("费率表 {}@{}", self.entry.vendor, self.entry.effective_from);
        match self.tier {
            Tier::Flat => format!("{base} · 统一价"),
            Tier::Peak => format!("{base} · 高峰价"),
            Tier::OffPeak => format!("{base} · 空闲价 ×{}", self.multiplier),
        }
    }
}

fn model_matches(model: &str, mr: &ModelRate) -> bool {
    model == mr.name || model.ends_with(&mr.name)
}

/// 解析版本/事件时间：RFC3339 与纯日期（YYYY-MM-DD，按 UTC 00:00）都支持。
/// 解析失败返回 None——调用方回退到字典序，保持旧费率表的行为。
fn ts_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc().timestamp())
}

/// 时间先后：优先按时间点比较（避免 "Z" 与 "+08:00" 混排时字典序失真），
/// 任一侧不可解析时退回字符串比较。
fn ts_cmp(a: &str, b: &str) -> Ordering {
    match (ts_secs(a), ts_secs(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
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
        .filter(|v| ts_cmp(&v.effective_from, ev_ts) != Ordering::Greater)
        .max_by(|a, b| ts_cmp(&a.effective_from, &b.effective_from))
        .or_else(|| {
            applicable
                .iter()
                .copied()
                .min_by(|a, b| ts_cmp(&a.effective_from, &b.effective_from))
        })
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    (h <= 23 && m <= 59).then_some(h * 3600 + m * 60)
}

/// "09:00-12:00" → (开始秒, 结束秒)；start > end 表示跨午夜窗口。
fn parse_window(w: &str) -> Option<(u32, u32)> {
    let (a, b) = w.split_once('-')?;
    Some((parse_hhmm(a)?, parse_hhmm(b)?))
}

/// 判定事件时间落在高峰还是空闲。事件时间不可解析时按高峰计（宁可高估不低估）。
fn tier_at(tt: &TimeTiers, ev_ts: &str) -> Tier {
    let Ok(dt) = DateTime::parse_from_rfc3339(ev_ts.trim()) else {
        return Tier::Peak;
    };
    let offset = FixedOffset::east_opt(tt.utc_offset_minutes.saturating_mul(60))
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC 偏移恒有效"));
    let local = dt.with_timezone(&offset);
    if tt
        .holiday_dates
        .iter()
        .any(|d| d == &local.date_naive().format("%Y-%m-%d").to_string())
    {
        return Tier::OffPeak;
    }
    if tt.weekdays_only && matches!(local.weekday(), Weekday::Sat | Weekday::Sun) {
        return Tier::OffPeak;
    }
    let secs = local.time().num_seconds_from_midnight();
    let mut parsed_any = false;
    for w in &tt.peak_windows {
        if let Some((start, end)) = parse_window(w) {
            parsed_any = true;
            let inside = if start <= end {
                secs >= start && secs < end
            } else {
                secs >= start || secs < end
            };
            if inside {
                return Tier::Peak;
            }
        }
    }
    // 窗口全部无法解析时按高峰计（保守），否则窗口外即空闲
    if parsed_any {
        Tier::OffPeak
    } else {
        Tier::Peak
    }
}

/// 返回 (模型, 事件时间) 的实际单价：含版本选择与时段倍数。
/// estimate() 与账单分项计价共用这一入口，保证两处口径一致。
pub fn price_at<'a>(model: &str, ev_ts: &str, table: &'a [VendorRates]) -> Option<Price<'a>> {
    let entry = best_entry(table, model, ev_ts)?;
    let model_rate = entry.models.iter().find(|mr| model_matches(model, mr))?;
    let (tier, multiplier) = match &entry.time_tiers {
        None => (Tier::Flat, 1.0),
        Some(tt) => {
            let t = tier_at(tt, ev_ts);
            let m = if t == Tier::OffPeak {
                tt.off_peak_multiplier
            } else {
                1.0
            };
            (t, m)
        }
    };
    Some(Price {
        entry,
        model: model_rate,
        tool_requests: entry.tool_requests.as_ref(),
        tier,
        multiplier,
    })
}

/// 费率快照指纹：对（已按厂商+生效版本排序的）条目原文做 sha256。
/// 账单把该值连同条目快照一起冻结，日后可确认"这张单当时按哪张表算的"。
pub fn snapshot_fingerprint(entries: &[VendorRates]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for e in entries {
        // 结构体字段顺序固定 → JSON 序列化稳定；价格均为有限浮点
        h.update(serde_json::to_vec(e).unwrap_or_default());
        h.update(b"\n");
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 解析费率 YAML（单条目或序列均可），load_dir 与出厂内嵌回退共用。
pub fn parse_yaml(s: &str) -> Vec<VendorRates> {
    if let Ok(vs) = serde_yaml::from_str::<Vec<VendorRates>>(s) {
        return vs;
    }
    serde_yaml::from_str::<VendorRates>(s)
        .map(|v| vec![v])
        .unwrap_or_default()
}

pub fn load_dir(dir: &Path) -> Vec<VendorRates> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("yaml") {
                if let Ok(s) = fs::read_to_string(&p) {
                    out.extend(parse_yaml(&s));
                }
            }
        }
    }
    out
}

/// 按事件估算成本：先按时间锁定生效版本与价档，再算 token 价 + 该版本内的工具按次价。
/// 找不到模型单价时只计工具按次部分（沿用旧厂商级兜底）。
pub fn estimate(ev: &LedgerEvent, table: &[VendorRates]) -> f64 {
    let Some(model) = &ev.model else {
        return tool_cost(ev, table);
    };
    let Some(p) = price_at(model, &ev.ts, table) else {
        return tool_cost(ev, table);
    };
    let mut cost = ev.tokens_in as f64 * p.input_per_mtok() / 1e6
        + ev.tokens_out as f64 * p.output_per_mtok() / 1e6
        + ev.cache_read as f64 * p.cache_read_per_mtok() / 1e6;
    // Search/Fetch 是独立的按次 API，官方定价页未给时段折扣，故不乘时段倍数
    if let Some(tr) = p.tool_requests {
        cost += ev.requests_search as f64 * tr.search_per_k / 1000.0
            + ev.requests_fetch as f64 * tr.fetch_per_k / 1000.0;
    } else {
        cost += tool_cost(ev, table);
    }
    cost
}

/// 无模型信息（纯 Search/Fetch 按次）事件适用的费率条目：按事件时间选生效版本。
/// v0.1 简化：账本不记厂商，多条厂商都配了按次价时取生效时间最新的那一条。
pub fn tool_entry_at<'a>(ev_ts: &str, table: &'a [VendorRates]) -> Option<&'a VendorRates> {
    let applicable: Vec<&VendorRates> = table
        .iter()
        .filter(|v| v.tool_requests.is_some())
        .collect();
    applicable
        .iter()
        .copied()
        .filter(|v| ts_cmp(&v.effective_from, ev_ts) != Ordering::Greater)
        .max_by(|a, b| ts_cmp(&a.effective_from, &b.effective_from))
        .or_else(|| {
            applicable
                .iter()
                .copied()
                .min_by(|a, b| ts_cmp(&a.effective_from, &b.effective_from))
        })
}

/// 无模型信息的事件兜底：按事件时间取生效版本的按次价（v0.1 简化：不记厂商）。
fn tool_cost(ev: &LedgerEvent, table: &[VendorRates]) -> f64 {
    if ev.requests_search + ev.requests_fetch == 0 {
        return 0.0;
    }
    let Some(v) = tool_entry_at(&ev.ts, table) else {
        return 0.0;
    };
    let Some(tr) = &v.tool_requests else {
        return 0.0;
    };
    ev.requests_search as f64 * tr.search_per_k / 1000.0
        + ev.requests_fetch as f64 * tr.fetch_per_k / 1000.0
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
            time_tiers: None,
            models: vec![ModelRate { name: "deepseek-chat".into(), input_per_mtok: input, output_per_mtok: output, cache_read_per_mtok: 0.0 }],
            tool_requests: Some(ToolRequestRate { search_per_k: per_k, fetch_per_k: per_k }),
        }
    }

    /// DeepSeek 官方口径：北京时间周一至周五 09:00-12:00、14:00-18:00 高峰，空闲半价。
    fn tiered(eff: &str) -> VendorRates {
        let mut v = entry(eff, 2.0, 8.0, 5.0);
        v.time_tiers = Some(TimeTiers {
            peak_windows: vec!["09:00-12:00".into(), "14:00-18:00".into()],
            utc_offset_minutes: 480,
            weekdays_only: true,
            holiday_dates: vec![],
            off_peak_multiplier: 0.5,
        });
        v
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
    fn price_at_returns_matching_entry_and_none_when_absent() {
        let table = vec![entry("2026-09-01", 1.0, 2.0, 5.0), entry("2026-11-01", 2.0, 3.0, 8.0)];
        let p = price_at("deepseek-chat", "2026-10-15T00:00:00Z", &table).unwrap();
        assert!((p.input_per_mtok() - 1.0).abs() < 1e-9);
        assert!((p.tool_requests.unwrap().search_per_k - 5.0).abs() < 1e-9);
        assert_eq!(p.tier, Tier::Flat);
        assert!(price_at("no-such-model", "2026-10-15T00:00:00Z", &table).is_none());
    }

    /// 回归（2026-11-04）：版本与事件都带时区偏移时，字典序判断先后会失真。
    /// 版本 "2026-11-01T09:00:00+08:00" = 01:00Z；事件 00:00Z 在它之前、02:00Z 在它之后。
    #[test]
    fn version_selection_compares_instants_not_strings() {
        let table = vec![
            entry("2026-01-01", 1.0, 1.0, 0.0),
            entry("2026-11-01T09:00:00+08:00", 9.0, 9.0, 0.0),
        ];
        let mut before = ev();
        before.ts = "2026-11-01T00:00:00Z".into();
        assert!((price_at("deepseek-chat", &before.ts, &table).unwrap().input_per_mtok() - 1.0).abs() < 1e-9);
        let mut after = ev();
        after.ts = "2026-11-01T02:00:00Z".into();
        assert!((price_at("deepseek-chat", &after.ts, &table).unwrap().input_per_mtok() - 9.0).abs() < 1e-9);
    }

    /// 无模型事件（纯 Search/Fetch）也要按事件时间选按次价版本，与 token 计价同一时间口径。
    #[test]
    fn tool_only_uses_version_effective_at_event_time() {
        let table = vec![entry("2026-09-01", 1.0, 1.0, 5.0), entry("2026-11-01", 1.0, 1.0, 8.0)];
        let mut e = ev();
        e.model = None;
        e.requests_fetch = 0;
        e.ts = "2026-10-15T00:00:00Z".into();
        assert!((estimate(&e, &table) - 5.0).abs() < 1e-9);
        e.ts = "2026-11-05T00:00:00Z".into();
        assert!((estimate(&e, &table) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn off_peak_halves_token_price_but_not_per_request() {
        let table = vec![tiered("2026-01-01")];
        // 周二 10:00 北京（02:00Z）→ 高峰：2.0 + 8.0 + 按次 5.0
        let mut peak = ev();
        peak.ts = "2026-11-03T02:00:00Z".into();
        let p = price_at("deepseek-chat", &peak.ts, &table).unwrap();
        assert_eq!(p.tier, Tier::Peak);
        assert!((estimate(&peak, &table) - (2.0 + 8.0 + 5.0)).abs() < 1e-9);
        // 周二 23:30 北京（15:30Z）→ 空闲：token 减半，Search 按次价不变
        let mut idle = ev();
        idle.ts = "2026-11-03T15:30:00Z".into();
        let p = price_at("deepseek-chat", &idle.ts, &table).unwrap();
        assert_eq!(p.tier, Tier::OffPeak);
        assert!((p.input_per_mtok() - 1.0).abs() < 1e-9);
        assert!((estimate(&idle, &table) - (1.0 + 4.0 + 5.0)).abs() < 1e-9);
    }

    /// 边界：09:00 整点是高峰起点（含），12:00 是终点（不含）。
    #[test]
    fn peak_window_boundaries_are_half_open() {
        let table = vec![tiered("2026-01-01")];
        let at = |ts: &str| price_at("deepseek-chat", ts, &table).unwrap().tier;
        // 2026-11-03 是周二
        assert_eq!(at("2026-11-03T00:59:59Z"), Tier::OffPeak); // 08:59:59 北京
        assert_eq!(at("2026-11-03T01:00:00Z"), Tier::Peak); // 09:00:00
        assert_eq!(at("2026-11-03T03:59:59Z"), Tier::Peak); // 11:59:59
        assert_eq!(at("2026-11-03T04:00:00Z"), Tier::OffPeak); // 12:00:00
        assert_eq!(at("2026-11-03T06:00:00Z"), Tier::Peak); // 14:00:00
        assert_eq!(at("2026-11-03T10:00:00Z"), Tier::OffPeak); // 18:00:00
    }

    /// 周末全天按空闲价（2026-11-07 是周六）。
    #[test]
    fn weekend_is_off_peak_all_day() {
        let table = vec![tiered("2026-01-01")];
        let at = |ts: &str| price_at("deepseek-chat", ts, &table).unwrap().tier;
        assert_eq!(at("2026-11-07T02:00:00Z"), Tier::OffPeak); // 周六 10:00 北京
    }

    /// 法定节假日需显式配置：配置里列出的日期全天按空闲价。
    #[test]
    fn configured_holiday_is_off_peak() {
        let mut v = tiered("2026-01-01");
        v.time_tiers.as_mut().unwrap().holiday_dates = vec!["2026-11-03".into()];
        let table = vec![v];
        let at = |ts: &str| price_at("deepseek-chat", ts, &table).unwrap().tier;
        assert_eq!(at("2026-11-03T02:00:00Z"), Tier::OffPeak, "节假日 10:00 北京也应空闲");
    }

    #[test]
    fn source_mentions_tier_and_version() {
        let table = vec![tiered("2026-09-01")];
        let p = price_at("deepseek-chat", "2026-11-03T02:00:00Z", &table).unwrap();
        let s = p.source();
        assert!(s.contains("deepseek@2026-09-01"), "{s}");
        assert!(s.contains("高峰"), "{s}");
    }
}
