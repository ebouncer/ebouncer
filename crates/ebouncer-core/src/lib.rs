// SPDX-License-Identifier: AGPL-3.0-only
//! eBouncer 核心库。数据契约见仓库根 `schemas/`（唯一事实源，本模块结构与其对齐）。

pub mod audit;
pub mod attrib;
pub mod bill;
pub mod ledger;
pub mod rates;
pub mod redact;
pub mod rules;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
