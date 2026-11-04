// SPDX-License-Identifier: AGPL-3.0-only
//! 可携带证据包（`.ebc.zip`）：账单 + 账本切片 + 费率快照 + 可选审计链。
//!
//! 目标：客户/审计方**不装 eBouncer、不读本机 `~/.ebouncer`** 也能验证：
//! 1) 文件未被改（manifest + SHA256SUMS 双重哈希）；
//! 2) 账单金额可由包内账本与费率复算出来；
//! 3) 费率指纹与快照一致；
//! 4) 若带审计链，链与尾部 checkpoint 自洽。
//!
//! ZIP 用 store-only 自实现（不引入额外依赖；证据包本身不需要压缩）。
//! 这是"证据"不是"公证"：它证明包内自洽，不证明事件本身真实。

use crate::audit;
use crate::bill::{self, Invoice};
use crate::ledger::LedgerEvent;
use crate::rates::{self, VendorRates};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub created_at: String,
    pub ebc_version: String,
    pub client: String,
    pub period: crate::bill::Period,
    pub files: Vec<FileEntry>,
    pub invoice_total: f64,
    pub currency: String,
    /// 生成时完整账本的 sha256（= invoice.data_basis；包内只含切片，无法独立复算）
    pub ledger_sha256: Option<String>,
    pub ledger_slice_sha256: String,
    pub rates_fingerprint: String,
    /// 是否包含并校验了审计链（有 audit.jsonl 即 true；有 checkpoint 才会校验截断）
    pub audit_checked: bool,
}

pub struct BundleRequest<'a> {
    pub client: &'a str,
    /// 已按窗口过滤的事件（含其他客户；内部再按 client 过滤）
    pub events: &'a [LedgerEvent],
    pub table: &'a [VendorRates],
    pub markup: f64,
    pub watermark: bool,
    pub data_basis: Option<String>,
    /// 订阅月费（直通/分摊）；空 = 纯按量
    pub subscriptions: &'a [bill::SubscriptionFee],
    /// 数据目录（含 audit.jsonl / audit.head）；None = 不打包审计
    pub audit_dir: Option<&'a Path>,
    pub ebc_version: &'a str,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub client: String,
    pub total: f64,
    pub currency: String,
    pub lines: usize,
    pub audit_checked: bool,
    pub warnings: Vec<String>,
}

fn sha256(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_ledger(text: &str) -> Result<Vec<LedgerEvent>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let ev: LedgerEvent = serde_json::from_str(line)
            .map_err(|e| format!("ledger.jsonl 第 {} 行解析失败：{e}", i + 1))?;
        out.push(ev);
    }
    Ok(out)
}

/// 生成证据包：返回 zip 字节与 manifest。
pub fn create(req: &BundleRequest) -> Result<(Vec<u8>, Manifest), String> {
    let slice: Vec<LedgerEvent> = req
        .events
        .iter()
        .filter(|e| e.client.as_deref() == Some(req.client))
        .cloned()
        .collect();
    if slice.is_empty() {
        return Err(format!("客户「{}」在该周期内没有账目", req.client));
    }
    let invoice = bill::build_with_subscriptions(
        req.client,
        req.events,
        req.table,
        req.markup,
        req.watermark,
        req.data_basis.clone(),
        req.subscriptions,
    )
    .map_err(|e| e.to_string())?;

    let mut ledger_text = String::new();
    for ev in &slice {
        ledger_text.push_str(&serde_json::to_string(ev).map_err(|e| e.to_string())?);
        ledger_text.push('\n');
    }
    let invoice_json = serde_json::to_string_pretty(&invoice).map_err(|e| e.to_string())?;
    let rates_json =
        serde_json::to_string_pretty(&invoice.rates_snapshot).map_err(|e| e.to_string())?;

    let mut payload: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    payload.insert("invoice.json".into(), invoice_json.into_bytes());
    payload.insert("ledger.jsonl".into(), ledger_text.clone().into_bytes());
    payload.insert("rates.json".into(), rates_json.into_bytes());

    // 审计链可选：默认不带（命令文本虽已脱敏，仍属敏感证据）
    let mut audit_checked = false;
    if let Some(dir) = req.audit_dir
        && let Ok(bytes) = fs::read(dir.join("audit.jsonl"))
    {
        payload.insert("audit.jsonl".into(), bytes);
        audit_checked = true;
        if let Ok(head) = fs::read(dir.join("audit.head")) {
            payload.insert("audit.head".into(), head);
        }
    }

    let readme = format!(
        "eBouncer 证据包 v{}\n\
客户: {}\n\
周期: {} ~ {}\n\
账单: {} {}（含加成 {:.0}%）\n\
包含: manifest.json, invoice.json, ledger.jsonl, rates.json{}\n\
\n\
验证步骤（二选一）：\n\
1) 解压后在本目录执行: sha256sum -c SHA256SUMS\n\
2) 装 eBouncer 后执行: ebc verify-bundle <本文件>\n\
\n\
说明：金额是估算口径（见 invoice.json 的 price_source / note）；\n\
复算使用包内账本与费率快照，不依赖生成者的本机数据。\n",
        SCHEMA_VERSION,
        req.client,
        invoice.period.from,
        invoice.period.to,
        invoice.currency,
        invoice.total,
        invoice.markup * 100.0,
        if audit_checked {
            " + audit.jsonl/audit.head"
        } else {
            ""
        },
    );
    payload.insert("README.txt".into(), readme.into_bytes());

    let files: Vec<FileEntry> = payload
        .iter()
        .map(|(path, data)| FileEntry {
            path: path.clone(),
            sha256: sha256(data),
            bytes: data.len() as u64,
        })
        .collect();

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        created_at: Utc::now().to_rfc3339(),
        ebc_version: req.ebc_version.to_string(),
        client: req.client.to_string(),
        period: invoice.period.clone(),
        files,
        invoice_total: invoice.total,
        currency: invoice.currency.clone(),
        ledger_sha256: req.data_basis.clone(),
        ledger_slice_sha256: sha256(ledger_text.as_bytes()),
        rates_fingerprint: invoice.rates_fingerprint.clone(),
        audit_checked,
    };
    let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;

    // SHA256SUMS 覆盖 manifest + 所有 payload（不含自身），便于第三方 sha256sum -c
    let mut sums = String::new();
    sums.push_str(&format!(
        "{}  manifest.json\n",
        sha256(manifest_json.as_bytes())
    ));
    for (path, data) in &payload {
        sums.push_str(&format!("{}  {}\n", sha256(data), path));
    }

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    entries.push(("manifest.json".into(), manifest_json.into_bytes()));
    entries.push(("SHA256SUMS".into(), sums.into_bytes()));
    for (path, data) in payload {
        entries.push((path, data));
    }
    Ok((zip_store(&entries), manifest))
}

/// 验证证据包（只读文件，不碰调用者的数据目录）。
pub fn verify_bytes(bytes: &[u8]) -> Result<VerifyReport, String> {
    let files = zip_read(bytes)?;
    let manifest_raw = files.get("manifest.json").ok_or("包内缺少 manifest.json")?;
    let manifest: Manifest =
        serde_json::from_slice(manifest_raw).map_err(|e| format!("manifest.json 解析失败：{e}"))?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "不支持的 manifest 版本 {}（本工具支持 {}）",
            manifest.schema_version, SCHEMA_VERSION
        ));
    }

    // 1) SHA256SUMS 自检（第三方可不装 ebc 直接验）
    let sums_raw = files.get("SHA256SUMS").ok_or("包内缺少 SHA256SUMS")?;
    let sums_text = std::str::from_utf8(sums_raw).map_err(|_| "SHA256SUMS 不是 UTF-8")?;
    let mut checked = 0usize;
    for line in sums_text.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line
            .split_once("  ")
            .ok_or_else(|| format!("SHA256SUMS 行格式错误：{line}"))?;
        let data = files
            .get(name)
            .ok_or_else(|| format!("SHA256SUMS 列出的 {name} 不在包内"))?;
        if sha256(data) != hash {
            return Err(format!("{name} 的 sha256 与 SHA256SUMS 不符"));
        }
        checked += 1;
    }
    if checked == 0 {
        return Err("SHA256SUMS 为空".to_string());
    }

    // 2) manifest.files 自检
    for entry in &manifest.files {
        let data = files
            .get(&entry.path)
            .ok_or_else(|| format!("manifest 列出的 {} 不在包内", entry.path))?;
        if data.len() as u64 != entry.bytes {
            return Err(format!("{} 的大小与 manifest 不符", entry.path));
        }
        if sha256(data) != entry.sha256 {
            return Err(format!("{} 的 sha256 与 manifest 不符", entry.path));
        }
    }

    // 3) 用包内账本 + 费率复算账单
    let invoice_raw = files.get("invoice.json").ok_or("包内缺少 invoice.json")?;
    let invoice: Invoice =
        serde_json::from_slice(invoice_raw).map_err(|e| format!("invoice.json 解析失败：{e}"))?;
    let ledger_raw = files.get("ledger.jsonl").ok_or("包内缺少 ledger.jsonl")?;
    let ledger_text = std::str::from_utf8(ledger_raw).map_err(|_| "ledger.jsonl 不是 UTF-8")?;
    let events = parse_ledger(ledger_text)?;
    let rates_raw = files.get("rates.json").ok_or("包内缺少 rates.json")?;
    let table: Vec<VendorRates> =
        serde_json::from_slice(rates_raw).map_err(|e| format!("rates.json 解析失败：{e}"))?;

    if invoice.rates_fingerprint != rates::snapshot_fingerprint(&table) {
        return Err("费率指纹与 rates.json 不一致（快照可能被替换）".to_string());
    }
    if invoice.rates_fingerprint != manifest.rates_fingerprint {
        return Err("费率指纹与 manifest 不一致".to_string());
    }
    let subs: Vec<bill::SubscriptionFee> = invoice
        .subscriptions
        .iter()
        .map(|s| bill::SubscriptionFee {
            provider: s.provider.clone(),
            monthly: s.monthly,
            currency: s.currency.clone(),
            allocate: s.allocate,
        })
        .collect();
    let redo = bill::build_with_subscriptions(
        &invoice.client,
        &events,
        &table,
        invoice.markup,
        invoice.watermark,
        invoice.data_basis.clone(),
        &subs,
    )
    .map_err(|e| format!("复算失败：{e}"))?;

    if redo.currency != invoice.currency {
        return Err(format!(
            "复算币种 {} 与账单币种 {} 不一致",
            redo.currency, invoice.currency
        ));
    }
    if redo.items.len() != invoice.items.len() {
        return Err(format!(
            "复算分项数 {} 与账单 {} 不一致",
            redo.items.len(),
            invoice.items.len()
        ));
    }
    for (a, b) in redo.items.iter().zip(invoice.items.iter()) {
        if a.category != b.category || a.qty != b.qty {
            return Err(format!(
                "复算分项不一致：{} {} vs {} {}",
                a.category, a.qty, b.category, b.qty
            ));
        }
        if (a.unit_price_per_k_or_mtok - b.unit_price_per_k_or_mtok).abs() > 1e-9
            || (a.amount - b.amount).abs() > 1e-6
        {
            return Err(format!(
                "复算金额不一致：{} 单价 {} vs {}，金额 {} vs {}",
                a.category,
                a.unit_price_per_k_or_mtok,
                b.unit_price_per_k_or_mtok,
                a.amount,
                b.amount
            ));
        }
    }
    if (redo.total - invoice.total).abs() > 1e-6 {
        return Err(format!(
            "复算合计 {} 与账单合计 {} 不一致",
            redo.total, invoice.total
        ));
    }
    if (manifest.invoice_total - invoice.total).abs() > 1e-6
        || manifest.currency != invoice.currency
    {
        return Err("manifest 的金额/币种与 invoice.json 不一致".to_string());
    }

    // 4) 审计链（可选）
    let mut warnings = Vec::new();
    let mut audit_checked = false;
    if let Some(audit_raw) = files.get("audit.jsonl") {
        audit_checked = true;
        let tmp = std::env::temp_dir().join(format!(
            "ebc-bundle-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败：{e}"))?;
        let ap = tmp.join("audit.jsonl");
        fs::write(&ap, audit_raw).map_err(|e| format!("写临时审计文件失败：{e}"))?;
        if let Some(head) = files.get("audit.head") {
            fs::write(tmp.join("audit.head"), head)
                .map_err(|e| format!("写临时 checkpoint 失败：{e}"))?;
        } else {
            warnings.push(
                "包含 audit.jsonl 但没有 checkpoint：只能校验链内自洽，检测不了尾部截断"
                    .to_string(),
            );
        }
        let rep = audit::verify(&ap).map_err(|e| format!("审计链读取失败：{e}"))?;
        let _ = fs::remove_dir_all(&tmp);
        if !rep.is_ok() {
            if let Some((exp, got)) = rep.truncated {
                return Err(format!(
                    "审计链被截断：checkpoint 记录 {exp} 条，包内只有 {got} 条"
                ));
            }
            return Err(format!("审计链第 {} 行断链", rep.broken_at));
        }
        warnings.push(format!("审计链校验通过（{} 条）", rep.lines));
    }

    Ok(VerifyReport {
        client: invoice.client.clone(),
        total: invoice.total,
        currency: invoice.currency.clone(),
        lines: events.len(),
        audit_checked,
        warnings,
    })
}

// ---------- 最小 ZIP 读写（store-only，UTF-8 文件名） ----------

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn zip_store(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut meta: Vec<(u32, u32, u32, String)> = Vec::new(); // offset, crc, len, name
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let nb = name.as_bytes();
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(0x0800u16.to_le_bytes()); // UTF-8
        out.extend(0u16.to_le_bytes()); // store
        out.extend(0u16.to_le_bytes()); // time
        out.extend(0x0021u16.to_le_bytes()); // date 1980-01-01
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((nb.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(nb);
        out.extend(data);
        meta.push((offset, crc, data.len() as u32, name.clone()));
    }
    let cd_start = out.len() as u32;
    for (offset, crc, len, name) in &meta {
        let nb = name.as_bytes();
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(0x0800u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0x0021u16.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend((nb.len() as u16).to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u32.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(nb);
    }
    let cd_size = central.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend((meta.len() as u16).to_le_bytes());
    out.extend((meta.len() as u16).to_le_bytes());
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_start.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

pub fn zip_read(data: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    if data.len() < 22 {
        return Err("不是有效的 zip（文件过短）".to_string());
    }
    let start = data.len().saturating_sub(65557);
    let mut eocd = None;
    for i in (start..=data.len() - 4).rev() {
        if u32_at(data, i) == Some(0x0605_4b50) {
            eocd = Some(i);
            break;
        }
    }
    let eocd = eocd.ok_or("不是有效的 zip（找不到 EOCD）")?;
    let count = u16_at(data, eocd + 10).ok_or("zip EOCD 损坏")? as usize;
    let cd_off = u32_at(data, eocd + 16).ok_or("zip EOCD 损坏")? as usize;
    let mut pos = cd_off;
    let mut out = BTreeMap::new();
    for _ in 0..count {
        if u32_at(data, pos) != Some(0x0201_4b50) {
            return Err("zip 中央目录损坏".to_string());
        }
        let method = u16_at(data, pos + 10).ok_or("zip 中央目录损坏")?;
        if method != 0 {
            return Err("只支持 store（未压缩）zip".to_string());
        }
        let crc = u32_at(data, pos + 16).ok_or("zip 中央目录损坏")?;
        let usize_ = u32_at(data, pos + 24).ok_or("zip 中央目录损坏")? as usize;
        let nlen = u16_at(data, pos + 28).ok_or("zip 中央目录损坏")? as usize;
        let elen = u16_at(data, pos + 30).ok_or("zip 中央目录损坏")? as usize;
        let clen = u16_at(data, pos + 32).ok_or("zip 中央目录损坏")? as usize;
        let lho = u32_at(data, pos + 42).ok_or("zip 中央目录损坏")? as usize;
        let name = std::str::from_utf8(
            data.get(pos + 46..pos + 46 + nlen)
                .ok_or("zip 文件名越界")?,
        )
        .map_err(|_| "zip 文件名不是 UTF-8")?
        .to_string();
        let l_nlen = u16_at(data, lho + 26).ok_or("zip 本地头损坏")? as usize;
        let l_elen = u16_at(data, lho + 28).ok_or("zip 本地头损坏")? as usize;
        let ds = lho + 30 + l_nlen + l_elen;
        let payload = data
            .get(ds..ds + usize_)
            .ok_or_else(|| format!("zip 条目 {name} 数据越界"))?;
        if crc32(payload) != crc {
            return Err(format!("zip 条目 {name} CRC 校验失败"));
        }
        out.insert(name, payload.to_vec());
        pos += 46 + nlen + elen + clen;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(client: &str, ts: &str, tin: u64) -> LedgerEvent {
        LedgerEvent {
            ts: ts.into(),
            session_id: "s".into(),
            tool: "dsh".into(),
            project: "p".into(),
            client: Some(client.into()),
            model: Some("deepseek-flash".into()),
            tokens_in: tin,
            tokens_out: 0,
            cache_read: 0,
            cache_write: 0,
            cost_native: 0.0,
            currency: "CNY".into(),
            requests_search: 0,
            requests_fetch: 0,
        }
    }

    fn table() -> Vec<VendorRates> {
        vec![VendorRates {
            vendor: "deepseek".into(),
            currency: "CNY".into(),
            effective_from: "2026-11-01".into(),
            time_tiers: None,
            models: vec![crate::rates::ModelRate {
                name: "deepseek-flash".into(),
                input_per_mtok: 2.0,
                output_per_mtok: 8.0,
                cache_read_per_mtok: 0.04,
                cache_write_per_mtok: 0.0,
            }],
            tool_requests: None,
        }]
    }

    fn make_bundle() -> (Vec<u8>, Manifest) {
        let events = vec![
            ev("ACME", "2026-11-03T02:00:00Z", 1_000_000),
            ev("OTHER", "2026-11-03T02:00:00Z", 9_000_000),
        ];
        create(&BundleRequest {
            client: "ACME",
            events: &events,
            table: &table(),
            markup: 0.2,
            watermark: true,
            data_basis: Some("deadbeef".into()),
            subscriptions: &[],
            audit_dir: None,
            ebc_version: "0.1.0",
        })
        .unwrap()
    }

    #[test]
    fn zip_roundtrip_utf8_names() {
        let entries = vec![
            ("manifest.json".to_string(), b"{}".to_vec()),
            ("客户/账单.txt".to_string(), "你好".as_bytes().to_vec()),
        ];
        let z = zip_store(&entries);
        let back = zip_read(&z).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back["客户/账单.txt"], "你好".as_bytes());
    }

    #[test]
    fn bundle_roundtrip_verifies_and_recomputes() {
        let (z, m) = make_bundle();
        assert_eq!(m.invoice_total, 2.4); // 2.0 × 1.2
        let r = verify_bytes(&z).unwrap();
        assert_eq!(r.client, "ACME");
        assert!((r.total - 2.4).abs() < 1e-9);
        assert_eq!(r.lines, 1);
    }

    #[test]
    fn tampered_invoice_total_is_detected() {
        let (z, _) = make_bundle();
        let mut files = zip_read(&z).unwrap();
        let inv: serde_json::Value = serde_json::from_slice(&files["invoice.json"]).unwrap();
        let mut inv = inv;
        inv["total"] = serde_json::json!(0.01);
        files.insert(
            "invoice.json".into(),
            serde_json::to_vec_pretty(&inv).unwrap(),
        );
        let entries: Vec<(String, Vec<u8>)> = files.into_iter().collect();
        let z2 = zip_store(&entries);
        // manifest/SHA256SUMS 仍是旧哈希，先报哈希不符（这就是防篡改的第一道）
        let err = verify_bytes(&z2).unwrap_err();
        assert!(err.contains("sha256") || err.contains("复算"), "{err}");
    }

    #[test]
    fn tampered_ledger_without_rehash_is_detected() {
        let (z, _) = make_bundle();
        let mut files = zip_read(&z).unwrap();
        let mut text = String::from_utf8(files["ledger.jsonl"].clone()).unwrap();
        text = text.replace("1000000", "9999999");
        files.insert("ledger.jsonl".into(), text.into_bytes());
        let entries: Vec<(String, Vec<u8>)> = files.into_iter().collect();
        let z2 = zip_store(&entries);
        assert!(verify_bytes(&z2).is_err());
    }
}
