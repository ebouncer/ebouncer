// SPDX-License-Identifier: AGPL-3.0-only
//! 宿主日志解析器。契约测试原则（D2）：每个解析器配 tests/golden/ 真实样本，
//! 上游改版先照样本红、再适配——适配层是永续税，用测试把它钉住。
//!
//! ⚠️ 本模块的字段路径为待核验假设：W2 需用真实日志（~/.claude/projects、~/.codex/sessions）
//! 脱敏后作为 golden 样本入仓，逐字段核对后删除 TODO。禁止凭记忆定契约。

pub mod claude_code;
pub mod codex;

use ebouncer_core::ledger::LedgerEvent;

pub trait Adapter {
    /// 从单个日志文件解析出账本事件序列（容忍坏行，跳过即可）。
    fn parse_file(&self, path: &std::path::Path) -> Vec<LedgerEvent>;
}

/// 从项目目录名/工作目录路径归一化项目名。
pub fn normalize_project(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}
