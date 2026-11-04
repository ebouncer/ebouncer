// SPDX-License-Identifier: AGPL-3.0-only
//! eBouncer 核心库。数据契约见仓库根 `schemas/`（唯一事实源，本模块结构与其对齐）。

pub mod attrib;
pub mod audit;
pub mod bill;
pub mod bundle;
pub mod ledger;
pub mod plan;
pub mod rates;
pub mod redact;
pub mod render;
pub mod rules;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
