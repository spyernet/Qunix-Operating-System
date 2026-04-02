//! MAC policy persistence — runtime read/write via /proc/qsf/policy.
//!
//! Policy file format (one rule per line):
//!
//!   # comment
//!   mac enable
//!   mac disable
//!   mac default deny
//!   mac default allow
//!   allow subject=<label_hex> object=<label_hex> access=<access_list>
//!   deny  subject=<label_hex> object=<label_hex> access=<access_list>
//!   flush
//!
//! Access list: comma-separated tokens:
//!   read write exec net ipc signal ptrace admin all
//!
//! Label hex: 64-bit value, e.g. 0x0200000000000000 = SecurityLabel::USER
//!
//! Examples:
//!   allow subject=0x0200000000000000 object=0x0100000000000000 access=read
//!   deny  subject=0x0300000000000000 object=0x0100000000000000 access=all
//!   mac enable
//!   mac default deny
//!
//! The file is write-only from userspace (/proc/qsf/policy, mode 0200).
//! Reading /proc/qsf/policy-dump returns the current ruleset.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::security::{SecurityLabel, MacAccess, mac_add_rule, mac_enable, mac_disable, MAC_POLICY};

// ── Parser ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ParseError {
    UnknownCommand(String),
    MissingField(&'static str),
    InvalidHex(String),
    InvalidAccess(String),
}

fn parse_label(s: &str) -> Result<SecurityLabel, ParseError> {
    let s = s.trim();
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u64::from_str_radix(hex, 16)
        .map(SecurityLabel)
        .map_err(|_| ParseError::InvalidHex(s.to_string()))
}

fn parse_access(s: &str) -> Result<u32, ParseError> {
    let s = s.trim();
    if s == "all" { return Ok(MacAccess::ALL); }
    let mut bits = 0u32;
    for token in s.split(',') {
        bits |= match token.trim() {
            "read"    | "r" => MacAccess::READ,
            "write"   | "w" => MacAccess::WRITE,
            "exec"    | "x" => MacAccess::EXEC,
            "net"     | "n" => MacAccess::NET,
            "ipc"           => MacAccess::IPC,
            "signal"  | "s" => MacAccess::SIGNAL,
            "ptrace"        => MacAccess::PTRACE,
            "admin"   | "a" => MacAccess::ADMIN,
            "all"           => MacAccess::ALL,
            t => return Err(ParseError::InvalidAccess(t.to_string())),
        };
    }
    Ok(bits)
}

fn extract_kv<'a>(tokens: &'a [&'a str], key: &str) -> Option<&'a str> {
    let prefix = alloc::format!("{}=", key);
    tokens.iter().find_map(|t| t.strip_prefix(prefix.as_str()))
}

/// Process one policy line. Returns Ok(()) or an error description.
pub fn apply_line(line: &str) -> Result<(), ParseError> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') { return Ok(()); }

    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() { return Ok(()); }

    match tokens[0] {
        "mac" => {
            match tokens.get(1).copied().unwrap_or("") {
                "enable"  => mac_enable(),
                "disable" => mac_disable(),
                "default" => {
                    let action = tokens.get(2).copied().unwrap_or("allow");
                    let mut guard = MAC_POLICY.lock();
                    guard.default_deny = action == "deny";
                }
                "flush"   => { MAC_POLICY.lock().rules.clear(); }
                x => return Err(ParseError::UnknownCommand(x.to_string())),
            }
        }
        "flush" => { MAC_POLICY.lock().rules.clear(); }
        "allow" | "deny" => {
            let allow = tokens[0] == "allow";
            let subj_str = extract_kv(&tokens, "subject").ok_or(ParseError::MissingField("subject"))?;
            let obj_str  = extract_kv(&tokens, "object").ok_or(ParseError::MissingField("object"))?;
            let acc_str  = extract_kv(&tokens, "access").ok_or(ParseError::MissingField("access"))?;

            let subject = parse_label(subj_str)?;
            let object  = parse_label(obj_str)?;
            let access  = parse_access(acc_str)?;

            let mac_access = if allow { MacAccess(access) } else { MacAccess(0) };

            // Deny rules: remove permissions from existing rule or add rule with 0
            mac_add_rule(subject, object, mac_access);

            if !allow {
                // For deny: store a special rule where we invert the access
                // We store the denied access bits by convention:
                // "deny exec" → add rule with access = ALL & !exec
                let inverted = MacAccess(MacAccess::ALL & !access);
                mac_add_rule(subject, object, inverted);
            }
        }
        _ => return Err(ParseError::UnknownCommand(tokens[0].to_string())),
    }
    Ok(())
}

/// Parse and apply a multi-line policy string. Returns number of errors.
pub fn apply_policy_text(text: &str) -> (usize, usize) {
    let mut ok = 0usize; let mut err = 0usize;
    for line in text.lines() {
        match apply_line(line) {
            Ok(()) => ok += 1,
            Err(e) => {
                crate::klog!("QSF-MAC policy error: {:?}", e);
                err += 1;
            }
        }
    }
    (ok, err)
}

/// Dump current policy as text (for /proc/qsf/policy-dump).
pub fn dump_policy() -> Vec<u8> {
    let guard = MAC_POLICY.lock();
    let mut out = String::new();
    out.push_str("# Qunix Security Foundation — MAC Policy Dump\n");
    out.push_str(&alloc::format!("mac {}\n", if guard.enabled { "enable" } else { "disable" }));
    out.push_str(&alloc::format!("mac default {}\n", if guard.default_deny { "deny" } else { "allow" }));
    for rule in &guard.rules {
        out.push_str(&alloc::format!(
            "allow subject=0x{:016x} object=0x{:016x} access={}\n",
            rule.subject.0, rule.object.0,
            access_to_str(rule.access.0),
        ));
    }
    out.into_bytes()
}

fn access_to_str(bits: u32) -> String {
    if bits == MacAccess::ALL { return "all".to_string(); }
    let mut parts: Vec<&str> = Vec::new();
    if bits & MacAccess::READ   != 0 { parts.push("read"); }
    if bits & MacAccess::WRITE  != 0 { parts.push("write"); }
    if bits & MacAccess::EXEC   != 0 { parts.push("exec"); }
    if bits & MacAccess::NET    != 0 { parts.push("net"); }
    if bits & MacAccess::IPC    != 0 { parts.push("ipc"); }
    if bits & MacAccess::SIGNAL != 0 { parts.push("signal"); }
    if bits & MacAccess::PTRACE != 0 { parts.push("ptrace"); }
    if bits & MacAccess::ADMIN  != 0 { parts.push("admin"); }
    if parts.is_empty() { "none".to_string() } else { parts.join(",") }
}

// ── procfs write handler ──────────────────────────────────────────────────
//
// Called from procfs when userspace writes to /proc/qsf/policy.
// The entire write buffer is parsed as policy text.

pub fn handle_policy_write(data: &[u8]) -> Result<usize, crate::vfs::VfsError> {
    let text = match core::str::from_utf8(data) {
        Ok(t)  => t,
        Err(_) => return Err(22u32), // EINVAL
    };
    let (ok, errs) = apply_policy_text(text);
    crate::klog!("QSF-MAC: policy update — {} rules applied, {} errors", ok, errs);
    Ok(data.len())
}
