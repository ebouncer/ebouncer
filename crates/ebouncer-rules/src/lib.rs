// SPDX-License-Identifier: AGPL-3.0-only
//! 规则快照数据仓：编译进二进制的出厂规则，随版本号发布（规则集市网络效应的种子）。

/// 出厂危险命令基线（YAML 原文，规则加载器与 `ebc rules --dump` 共用）
pub const DANGEROUS_CMDS: &str = include_str!("../dangerous-cmds.v0.1.yaml");

/// 出厂费率表条目（deepseek 按次价格已核验；token 单价为占位，启用前须复核定价页）
pub mod rates {
    pub const DEEPSEEK: &str = include_str!("../rates/deepseek.yaml");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_yaml_loads() {
        let parsed: serde_yaml::Value = serde_yaml::from_str(DANGEROUS_CMDS).unwrap();
        let n = parsed["rules"].as_seq().unwrap().len();
        assert!(n >= 30, "基线至少 30 条，实际 {n}");
    }
    #[test]
    fn all_patterns_compile() {
        let parsed: serde_yaml::Value = serde_yaml::from_str(DANGEROUS_CMDS).unwrap();
        for r in parsed["rules"].as_seq().unwrap() {
            let pat = r["pattern"].as_str().unwrap();
            assert!(regex::Regex::new(pat).is_ok(), "非法正则: {pat}");
        }
    }
}
