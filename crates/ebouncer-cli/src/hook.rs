// SPDX-License-Identifier: AGPL-3.0-only
//! W4 命令防火墙（dry-run）：init 注入 / hook 执行体 / uninstall 清理。
//! 设计红线（PRD §10.2 + 方案 D1/D2）：
//! - 不 spawn 子 shell、不改 /bin/bash：zsh 用 preexec，bash 用 PROMPT_COMMAND 读 history 尾部；
//! - init 默认只打印将要注入的内容（diff 预览），--yes 才写 rc 文件；块带 BEGIN/END 标记可幂等更新；
//! - hook 永远 exit 0（dry-run 不拦截、不改变用户命令行为），任何内部失败静默放行（fail-open，D4）；
//! - 只记录命中命令（隐私最小化），落盘前 redact（D3），审计链 O(1) 尾读。

use ebouncer_core::{audit, redact, rules::RuleSet};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "# >>> eBouncer hook v1 >>>";
pub const END: &str = "# <<< eBouncer hook <<<";

pub fn rc_path(home: &Path, shell: &str) -> PathBuf {
    home.join(if shell == "zsh" { ".zshrc" } else { ".bashrc" })
}

pub fn snippet(shell: &str) -> String {
    if shell == "zsh" {
        // preexec：$2 为用户输入的原始命令行
        "\
autoload -Uz add-zsh-hook
_ebouncer_preexec() {
  [ -n \"$EBC_DISABLE\" ] && return 0
  case \"$2\" in ebc\\ *) return 0 ;; esac
  printf '%s' \"$2\" | command ebc hook
  return 0
}
add-zsh-hook -Uz preexec _ebouncer_preexec"
            .to_string()
    } else {
        // bash 无 preexec；PROMPT_COMMAND 在新提示符前记录上一条完整命令（延迟入账）
        "\
_ebouncer_prompt() {
  [ -n \"$EBC_DISABLE\" ] && return 0
  local _ebc_last
  _ebc_last=$(history 1 | sed 's/^[ ]*[0-9]*[ ]*//')
  case \"$_ebc_last\" in ebc\\ *) return 0 ;; esac
  [ -n \"$_ebc_last\" ] && printf '%s' \"$_ebc_last\" | command ebc hook
  return 0
}
PROMPT_COMMAND=\"_ebouncer_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}\""
            .to_string()
    }
}

/// 去掉 BEGIN..END 块（含标记行）。返回 (新内容, 是否移除过)。
pub fn strip_block(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    let mut removed = false;
    for line in text.lines() {
        if line.trim_end() == BEGIN {
            inside = true;
            removed = true;
            continue;
        }
        if line.trim_end() == END {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, removed)
}

fn block_for(shell: &str) -> String {
    format!("{BEGIN}\n{}\n{END}\n", snippet(shell))
}

/// ebc init：--yes 前只预览；注入幂等（已有块则原位替换）。
pub fn init(home: &Path, shell: &str, yes: bool) -> i32 {
    let rc = rc_path(home, shell);
    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    let (stripped, had) = strip_block(&existing);
    if !yes {
        println!("目标文件：{}（当前 {}）", rc.display(), if had { "已含 eBouncer 块，将原位更新" } else { "无 eBouncer 块，将追加" });
        println!("--- 将注入以下内容（预览，未写入）---\n{}", block_for(shell));
        println!("确认后用 `ebc init --shell {shell} --yes` 注入；EBC_DISABLE=1 可临时停用。");
        return 0;
    }
    let mut content = stripped;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push('\n');
    content.push_str(&block_for(shell));
    match std::fs::write(&rc, content) {
        Ok(_) => {
            println!("已注入 {}。立即生效：source {} 或开新终端。", rc.display(), rc.display());
            0
        }
        Err(e) => {
            eprintln!("写入失败：{e}");
            1
        }
    }
}

/// ebc uninstall：只删标记块；数据目录默认保留（审计链是用户的证据），--purge 才清可再生状态。
pub fn uninstall(home: &Path, data_dir: &Path, purge: bool) -> i32 {
    let mut touched = false;
    for shell in ["bash", "zsh"] {
        let rc = rc_path(home, shell);
        let Ok(existing) = std::fs::read_to_string(&rc) else { continue };
        let (stripped, removed) = strip_block(&existing);
        if removed {
            match std::fs::write(&rc, stripped) {
                Ok(_) => {
                    println!("已移除 {} 中的 eBouncer 块", rc.display());
                    touched = true;
                }
                Err(e) => {
                    eprintln!("移除 {} 失败：{e}", rc.display());
                    return 1;
                }
            }
        }
    }
    if !touched {
        println!("未发现已注入的 eBouncer 块（rc 未改动）。");
    }
    if purge {
        // 保留 audit/ledger/exports（证据），清 trial/config/rules 等可再生状态
        let mut cleared = 0;
        for name in ["trial.json", "config", "rules.yaml"] {
            let p = data_dir.join(name);
            if p.exists() && std::fs::remove_dir_all(&p).or_else(|_| std::fs::remove_file(&p)).is_ok() {
                cleared += 1;
            }
        }
        println!("已清理 {cleared} 项可再生状态；账本/审计链/导出保留于 {}", data_dir.display());
        println!("确认要连证据一起删除请手动 rm -rf {}（不可逆，故不提供开关）", data_dir.display());
    } else {
        println!("数据目录未动：{}（含审计链，卸载 ≠ 销毁证据）", data_dir.display());
    }
    0
}

#[cfg(unix)]
fn current_uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}
#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// ebc hook（由 rc 块每条命令调一次）：读 stdin 命令 → 规则匹配 → 命中才记审计 + stderr 提示。
/// 永远 exit 0；内部任何错误静默（D4 fail-open）。
pub fn hook(data_dir: &Path) -> i32 {
    let dir = data_dir.to_path_buf();
    let _ = std::panic::catch_unwind(move || hook_inner(&dir));
    0
}

fn hook_inner(data_dir: &Path) -> i32 {
    let mut cmd = String::new();
    if std::io::stdin().read_to_string(&mut cmd).is_err() {
        return 0;
    }
    let cmd = cmd.trim().to_string();
    if cmd.is_empty() || cmd.starts_with("ebc ") || std::env::var("EBC_DISABLE").is_ok() {
        return 0;
    }
    // 规则：用户 <data>/rules.yaml 优先，否则出厂内嵌
    let set = match std::fs::read_to_string(data_dir.join("rules.yaml")) {
        Ok(s) => RuleSet::from_yaml_str(&s),
        Err(_) => RuleSet::from_yaml_str(ebouncer_rules::DANGEROUS_CMDS),
    }
    .unwrap_or_default();
    if set.rules.is_empty() {
        return 0;
    }
    let (decision, rid) = set.evaluate(&cmd, true); // v0.1 恒 dry-run
    if decision == "allow" {
        return 0;
    }
    let note = rid
        .as_ref()
        .and_then(|id| set.rules.iter().find(|r| &r.id == id))
        .map(|r| r.note.clone())
        .unwrap_or_default();
    let ts_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ev = audit::AuditEvent {
        seq: 0,
        ts_ns,
        uid: current_uid(),
        tty: std::env::var("TTY").unwrap_or_else(|_| "unknown".into()),
        session: std::env::var("TMUX_PANE")
            .or_else(|_| std::env::var("TERM_SESSION_ID"))
            .unwrap_or_else(|_| format!("pid-{}", std::process::id())),
        cmd: redact::redact(&cmd), // D3：明文密钥不落盘
        rule_id: rid.clone(),
        decision: decision.clone(),
        key_ref: None, // 个人版单用户；组织 KMS 后端在 L4
        prev_hash: String::new(),
        hash: String::new(),
    };
    let _ = audit::append(&data_dir.join("audit.jsonl"), ev);
    let rid_s = rid.unwrap_or_else(|| "-".into());
    eprintln!("⚠️ eBouncer(dry-run) 命中 {decision} 级规则 [{rid_s}]：{cmd}");
    if !note.is_empty() {
        eprintln!("   说明：{note}｜开启拦截前会先经你确认，现在不拦截");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strip_roundtrip_and_idempotent() {
        let rc = "alias ll='ls'\nexport A=1\n";
        let injected = format!("{rc}\n{}", block_for("zsh"));
        let (stripped, removed) = strip_block(&injected);
        assert!(removed);
        assert!(stripped.contains("alias ll"));
        assert!(!stripped.contains("add-zsh-hook"));
        // 再次 strip 无块可删
        assert!(!strip_block(&stripped).1);
    }
    #[test]
    fn updates_in_place_not_duplicates() {
        let mut content = String::from("orig\n");
        let (s, _) = strip_block(&content);
        content = s;
        content.push_str(&block_for("bash"));
        let (_, removed2) = strip_block(&content);
        assert!(removed2);
        assert_eq!(content.matches(BEGIN).count(), 1);
    }
}
