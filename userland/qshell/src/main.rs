/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! qsh v2.0 — Qunix Shell
//! Modern, interactive, programmable shell for Qunix OS.
//! Features: syntax highlighting, autosuggestions, context-aware completion,
//!            persistent history, plugins, native fn syntax, job control.
#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use libsys::*;

// ── ANSI colors ───────────────────────────────────────────────────────────
const C_RST:  &str = "\x1b[0m";
const C_BOLD: &str = "\x1b[1m";
const C_DIM:  &str = "\x1b[2m";
const C_RED:  &str = "\x1b[31m";
const C_GRN:  &str = "\x1b[32m";
const C_YEL:  &str = "\x1b[33m";
const C_BLU:  &str = "\x1b[34m";
const C_MAG:  &str = "\x1b[35m";
const C_CYN:  &str = "\x1b[36m";
const C_BRED: &str = "\x1b[91m";
const C_BGRN: &str = "\x1b[92m";
const C_BYEL: &str = "\x1b[93m";
const C_BBLU: &str = "\x1b[94m";
const C_BCYN: &str = "\x1b[96m";
const C_GRAY: &str = "\x1b[90m";

fn out(s: &str) { write(STDOUT, s.as_bytes()); }
fn err(s: &str) { write(STDERR, s.as_bytes()); }

// ── Terminal raw mode ─────────────────────────────────────────────────────
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
// c_lflag bits
const ISIG:   u32 = 0x0001;
const ICANON: u32 = 0x0002;
const ECHO:   u32 = 0x0008;
const ECHOE:  u32 = 0x0010;
const ECHOK:  u32 = 0x0020;
const ECHONL: u32 = 0x0040;
const IEXTEN: u32 = 0x8000;
// c_iflag bits
const BRKINT: u32 = 0x0002;
const ICRNL:  u32 = 0x0100;
const IXON:   u32 = 0x0400;
const INPCK:  u32 = 0x0010;
const ISTRIP: u32 = 0x0020;
const VMIN:   usize = 6;
const VTIME:  usize = 5;

static mut ORIG_TERMIOS: [u8; 64] = [0u8; 64];
static mut RAW_MODE: bool = false;

fn term_raw() {
    unsafe {
        if ioctl(STDIN, TCGETS, ORIG_TERMIOS.as_mut_ptr() as u64) < 0 { return; }
        let mut raw = ORIG_TERMIOS;
        let mut c_iflag = u32::from_ne_bytes([raw[0],raw[1],raw[2],raw[3]]);
        let mut c_lflag = u32::from_ne_bytes([raw[12],raw[13],raw[14],raw[15]]);
        c_iflag &= !(BRKINT | ICRNL | IXON | INPCK | ISTRIP);
        c_lflag &= !(ECHO | ECHOE | ECHOK | ECHONL | ICANON | IEXTEN | ISIG);
        raw[0..4].copy_from_slice(&c_iflag.to_ne_bytes());
        raw[12..16].copy_from_slice(&c_lflag.to_ne_bytes());
        raw[17 + VMIN] = 1; raw[17 + VTIME] = 0;
        ioctl(STDIN, TCSETS, raw.as_ptr() as u64);
        RAW_MODE = true;
    }
}
fn term_restore() {
    unsafe { if RAW_MODE { ioctl(STDIN, TCSETS, ORIG_TERMIOS.as_ptr() as u64); RAW_MODE = false; } }
}
fn term_width() -> usize {
    let mut ws = [0u16; 4];
    let r = ioctl(STDOUT, 0x5413, ws.as_mut_ptr() as u64);
    if r >= 0 && ws[1] > 0 { ws[1] as usize } else { 80 }
}
fn read_byte() -> Option<u8> {
    let mut b = [0u8; 1]; if read(STDIN, &mut b) == 1 { Some(b[0]) } else { None }
}
fn read_more_bytes(buf: &mut [u8]) -> i64 { read(STDIN, buf) }

// ── History ───────────────────────────────────────────────────────────────
const HIST_MAX: usize = 50000;

struct History {
    entries:    Vec<String>,
    timestamps: Vec<i64>,
    path:       String,
    dirty:      bool,
}
impl History {
    fn new(path: &str) -> Self {
        History { entries: Vec::new(), timestamps: Vec::new(), path: path.to_string(), dirty: false }
    }
    fn load(&mut self) {
        let mut p = self.path.clone(); p.push('\0');
        let fd = open(p.as_bytes(), O_RDONLY, 0); if fd < 0 { return; }
        let mut buf = alloc::vec![0u8; 1<<20]; let n = read(fd as i32, &mut buf);
        close(fd as i32); if n <= 0 { return; }
        for line in String::from_utf8_lossy(&buf[..n as usize]).split('\n') {
            let line = line.trim(); if line.is_empty() { continue; }
            let (ts, cmd) = if line.starts_with(": ") {
                let rest = &line[2..];
                if let Some(p) = rest.find(":0;") {
                    (rest[..p].trim().parse::<i64>().unwrap_or(0), rest[p+3..].to_string())
                } else { (0, line.to_string()) }
            } else { (0, line.to_string()) };
            if !cmd.is_empty() { self.push_raw(cmd, ts); }
        }
    }
    fn push_raw(&mut self, cmd: String, ts: i64) {
        if let Some(pos) = self.entries.iter().rposition(|e| e == &cmd) {
            self.entries.remove(pos); self.timestamps.remove(pos);
        }
        self.entries.push(cmd); self.timestamps.push(ts);
        if self.entries.len() > HIST_MAX { self.entries.remove(0); self.timestamps.remove(0); }
    }
    fn push(&mut self, cmd: &str) {
        if cmd.trim().is_empty() { return; }
        let mut ts = [0i64; 2]; clock_gettime(0, &mut ts);
        self.push_raw(cmd.to_string(), ts[0]); self.dirty = true;
    }
    fn save(&self) {
        if !self.dirty { return; }
        let mut p = self.path.clone(); p.push('\0');
        let fd = open(p.as_bytes(), O_WRONLY|O_CREAT|O_TRUNC, 0o600); if fd < 0 { return; }
        let start = self.entries.len().saturating_sub(HIST_MAX);
        for (i, cmd) in self.entries[start..].iter().enumerate() {
            let ts = self.timestamps.get(start+i).copied().unwrap_or(0);
            let line = alloc::format!(": {}:0;{}\n", ts, cmd);
            write(fd as i32, line.as_bytes());
        }
        close(fd as i32);
    }
    fn search_back(&self, query: &str, from: usize) -> Option<usize> {
        let end = from.min(self.entries.len());
        for i in (0..end).rev() { if self.entries[i].contains(query) { return Some(i); } }
        None
    }
    fn prefix_search_back(&self, prefix: &str, from: usize) -> Option<usize> {
        let end = from.min(self.entries.len());
        for i in (0..end).rev() { if self.entries[i].starts_with(prefix) { return Some(i); } }
        None
    }
    fn suggestion(&self, prefix: &str) -> Option<String> {
        for e in self.entries.iter().rev() {
            if e.starts_with(prefix) && e != prefix { return Some(e.clone()); }
        }
        None
    }
    fn len(&self) -> usize { self.entries.len() }
    fn get(&self, i: usize) -> Option<&str> { self.entries.get(i).map(|s| s.as_str()) }
}

// ── Syntax highlighting ───────────────────────────────────────────────────
const KEYWORDS: &[&str] = &[
    "if","then","elif","else","fi","while","until","do","done",
    "for","in","case","esac","function","fn","return","break",
    "continue","export","unset","local","readonly","declare",
];

fn is_cmd_valid(cmd: &str, sh: &Shell) -> bool {
    if sh.is_builtin(cmd) || sh.aliases.contains_key(cmd) || sh.functions.contains_key(cmd) { return true; }
    if cmd.contains('/') {
        let mut st = Stat::default(); let mut p = cmd.to_string(); p.push('\0');
        return stat(p.as_bytes(), &mut st) >= 0;
    }
    if let Some(path) = sh.env.get("PATH") {
        for dir in path.split(':') {
            let mut p = alloc::format!("{}/{}\0", dir, cmd);
            let mut st = Stat::default();
            if stat(p.as_bytes(), &mut st) >= 0 && st.st_mode & 0o111 != 0 { return true; }
        }
    }
    false
}

fn highlight_line(line: &str, sh: &Shell) -> String {
    if line.is_empty() { return String::new(); }
    let mut out_s = String::new();
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut after_cmd_sep = true; // next word is in command position

    while i < n {
        match chars[i] {
            ' ' | '\t' => { out_s.push(chars[i]); i += 1; }
            '#' => {
                out_s.push_str(C_GRAY);
                while i < n { out_s.push(chars[i]); i += 1; }
                out_s.push_str(C_RST);
            }
            '\'' => {
                out_s.push_str(C_BYEL);
                out_s.push('\''); i += 1;
                while i < n && chars[i] != '\'' { out_s.push(chars[i]); i += 1; }
                if i < n { out_s.push('\''); i += 1; }
                out_s.push_str(C_RST); after_cmd_sep = false;
            }
            '"' => {
                out_s.push_str(C_BYEL);
                out_s.push('"'); i += 1;
                while i < n && chars[i] != '"' {
                    if chars[i] == '\\' && i+1 < n { out_s.push(chars[i]); out_s.push(chars[i+1]); i += 2; }
                    else { out_s.push(chars[i]); i += 1; }
                }
                if i < n { out_s.push('"'); i += 1; }
                out_s.push_str(C_RST); after_cmd_sep = false;
            }
            '|' | ';' | '&' => {
                out_s.push_str(C_MAG);
                out_s.push(chars[i]); i += 1;
                if i < n && (chars[i] == '|' || chars[i] == '&' || chars[i] == '>') { out_s.push(chars[i]); i += 1; }
                out_s.push_str(C_RST); after_cmd_sep = true;
            }
            '<' | '>' => {
                out_s.push_str(C_BCYN);
                out_s.push(chars[i]); i += 1;
                if i < n && (chars[i]=='>' || chars[i]=='<' || chars[i]=='&') { out_s.push(chars[i]); i += 1; }
                out_s.push_str(C_RST);
            }
            '$' => {
                out_s.push_str(C_CYN);
                out_s.push('$'); i += 1;
                if i < n && chars[i] == '{' {
                    out_s.push('{'); i += 1;
                    while i < n && chars[i] != '}' { out_s.push(chars[i]); i += 1; }
                    if i < n { out_s.push('}'); i += 1; }
                } else if i < n && chars[i] == '(' {
                    out_s.push('('); i += 1;
                    let mut d=1;
                    while i<n && d>0 { if chars[i]=='('{d+=1;}else if chars[i]==')'{d-=1;} out_s.push(chars[i]); i+=1; }
                } else {
                    while i < n && (chars[i].is_alphanumeric()||chars[i]=='_'||"?$!@*#".contains(chars[i])) {
                        out_s.push(chars[i]); i += 1;
                    }
                }
                out_s.push_str(C_RST); after_cmd_sep = false;
            }
            _ => {
                let start = i;
                while i < n && !matches!(chars[i], ' '|'\t'|'\n'|';'|'&'|'|'|'<'|'>'|'#') {
                    if chars[i]=='\\' && i+1<n { i+=2; }
                    else if chars[i]=='\''||chars[i]=='"' { break; }
                    else { i+=1; }
                }
                let word: String = chars[start..i].iter().collect();
                if after_cmd_sep {
                    if KEYWORDS.contains(&word.as_str()) {
                        out_s.push_str(C_BBLU); out_s.push_str(&word); out_s.push_str(C_RST);
                    } else if is_cmd_valid(&word, sh) {
                        out_s.push_str(C_BGRN); out_s.push_str(&word); out_s.push_str(C_RST);
                    } else {
                        out_s.push_str(C_BRED); out_s.push_str(&word); out_s.push_str(C_RST);
                    }
                    after_cmd_sep = false;
                } else if word.starts_with('-') {
                    out_s.push_str(C_YEL); out_s.push_str(&word); out_s.push_str(C_RST);
                } else {
                    out_s.push_str(&word);
                }
            }
        }
    }
    out_s
}

// ── Completion ────────────────────────────────────────────────────────────
#[derive(Clone)]
struct Comp { display: String, insert: String, kind: CompKind }

#[derive(Clone, PartialEq)]
enum CompKind { Cmd, Dir, File, Var, Pid, Alias, Builtin }

fn complete_at(line: &str, cursor: usize, sh: &Shell) -> Vec<Comp> {
    let before = &line[..cursor];
    let word_start = before.rfind(|c: char| c==' '||c=='\t'||c=='|'||c==';'||c=='('||c=='&')
        .map(|i| i+1).unwrap_or(0);
    let partial = &before[word_start..];

    let trimmed = before[..word_start].trim_end();
    let is_first = trimmed.is_empty() || trimmed.ends_with('|') || trimmed.ends_with(';')
        || trimmed.ends_with('&') || trimmed.ends_with("&&") || trimmed.ends_with("||");

    let mut results = Vec::new();

    if partial.starts_with('$') {
        let pfx = &partial[1..];
        for (k,_) in &sh.env { if k.starts_with(pfx) { results.push(Comp{display:alloc::format!("${}",k),insert:alloc::format!("${}",k),kind:CompKind::Var}); } }
        return results;
    }

    if is_first {
        // Commands
        for b in BUILTINS { if b.starts_with(partial) { results.push(Comp{display:b.to_string(),insert:b.to_string(),kind:CompKind::Builtin}); } }
        for k in KEYWORDS  { if k.starts_with(partial) { results.push(Comp{display:k.to_string(),insert:k.to_string(),kind:CompKind::Cmd}); } }
        for (k,_) in &sh.aliases   { if k.starts_with(partial) { results.push(Comp{display:k.clone(),insert:k.clone(),kind:CompKind::Alias}); } }
        for (k,_) in &sh.functions { if k.starts_with(partial) { results.push(Comp{display:k.clone(),insert:k.clone(),kind:CompKind::Cmd}); } }
        // PATH scan
        if let Some(path_env) = sh.env.get("PATH") {
            for dir in path_env.split(':') {
                let dp = alloc::format!("{}\0", dir);
                let fd = open(dp.as_bytes(), O_RDONLY|O_DIRECTORY, 0); if fd < 0 { continue; }
                let mut buf = alloc::vec![0u8; 32768];
                loop {
                    let n = getdents64(fd as i32, &mut buf); if n<=0{break;}
                    let mut off=0;
                    while off < n as usize {
                        let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                        let nb=&buf[off+19..]; let nl=nb.iter().position(|&b|b==0).unwrap_or(0);
                        let name=String::from_utf8_lossy(&nb[..nl]).to_string();
                        if name.starts_with(partial) && !results.iter().any(|c|c.insert==name) {
                            let mut st=Stat::default(); let fp=alloc::format!("{}/{}\0",dir,name);
                            if stat(fp.as_bytes(),&mut st)>=0 && st.st_mode&0o111!=0 {
                                results.push(Comp{display:name.clone(),insert:name,kind:CompKind::Cmd});
                            }
                        }
                        if reclen==0{break;} off+=reclen;
                    }
                }
                close(fd as i32);
            }
        }
    } else {
        // Check context: cd → dirs only, kill → pids
        let prev_cmd = before[..word_start].trim_end().split_whitespace().last().unwrap_or("");
        let dir_only = matches!(prev_cmd, "cd"|"pushd"|"rmdir"|"mkdir");
        let pid_mode = matches!(prev_cmd, "kill"|"killall"|"wait");

        if pid_mode {
            let fd = open(b"/proc\0", O_RDONLY|O_DIRECTORY, 0);
            if fd >= 0 {
                let mut buf=alloc::vec![0u8;16384];
                loop {
                    let n=getdents64(fd as i32,&mut buf); if n<=0{break;}
                    let mut off=0;
                    while off<n as usize {
                        let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                        let nb=&buf[off+19..]; let nl=nb.iter().position(|&b|b==0).unwrap_or(0);
                        let name=String::from_utf8_lossy(&nb[..nl]).to_string();
                        if name.chars().all(|c|c.is_ascii_digit()) && name.starts_with(partial) {
                            results.push(Comp{display:name.clone(),insert:name,kind:CompKind::Pid});
                        }
                        if reclen==0{break;} off+=reclen;
                    }
                }
                close(fd as i32);
            }
        } else {
            // File/dir completion
            let (dir_part, file_part) = if let Some(sl)=partial.rfind('/') { (&partial[..sl+1],&partial[sl+1..]) } else { ("",partial) };
            let search = if dir_part.is_empty() { ".".to_string() } else { dir_part.trim_end_matches('/').to_string() };
            let sp = alloc::format!("{}\0", search);
            let fd = open(sp.as_bytes(), O_RDONLY|O_DIRECTORY, 0);
            if fd >= 0 {
                let mut buf=alloc::vec![0u8;32768];
                loop {
                    let n=getdents64(fd as i32,&mut buf); if n<=0{break;}
                    let mut off=0;
                    while off<n as usize {
                        let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                        let dtype=buf[off+18];
                        let nb=&buf[off+19..]; let nl=nb.iter().position(|&b|b==0).unwrap_or(0);
                        let name=String::from_utf8_lossy(&nb[..nl]).to_string();
                        let is_dir=dtype==4;
                        if (!name.starts_with('.')||file_part.starts_with('.')) && name.starts_with(file_part) {
                            if dir_only && !is_dir { if reclen==0{break;} off+=reclen; continue; }
                            let full = if dir_part.is_empty(){name.clone()}else{alloc::format!("{}{}",dir_part,name)};
                            let ins  = if is_dir{alloc::format!("{}/",full)}else{full};
                            let disp = if is_dir{alloc::format!("{}/",name)}else{name};
                            results.push(Comp{display:disp,insert:ins,kind:if is_dir{CompKind::Dir}else{CompKind::File}});
                        }
                        if reclen==0{break;} off+=reclen;
                    }
                }
                close(fd as i32);
            }
        }
    }

    results.sort_by(|a,b| a.display.cmp(&b.display));
    results.dedup_by(|a,b| a.insert==b.insert);
    results
}

fn comp_prefix(items: &[Comp]) -> String {
    if items.is_empty() { return String::new(); }
    let first=items[0].insert.as_bytes(); let mut len=first.len();
    for item in &items[1..] { let b=item.insert.as_bytes(); len=len.min(b.len()); while len>0&&&first[..len]!=&b[..len]{len-=1;} }
    String::from_utf8_lossy(&first[..len]).to_string()
}

// ── Line editor ───────────────────────────────────────────────────────────
enum ReadResult { Line(String), Eof }

struct Editor {
    buf:       Vec<u8>,
    cursor:    usize,
    hist_idx:  Option<usize>,
    saved:     Vec<u8>,
    srch_mode: bool,
    srch_q:    String,
    srch_idx:  Option<usize>,
    width:     usize,
}

impl Editor {
    fn new() -> Self {
        Editor { buf:Vec::new(), cursor:0, hist_idx:None, saved:Vec::new(),
                 srch_mode:false, srch_q:String::new(), srch_idx:None, width:80 }
    }
    fn line(&self) -> String { String::from_utf8_lossy(&self.buf).to_string() }
    fn insert_ch(&mut self, c: u8) { self.buf.insert(self.cursor,c); self.cursor+=1; }
    fn insert_str(&mut self, s: &str) { for b in s.bytes() { self.buf.insert(self.cursor,b); self.cursor+=1; } }
    fn del_back(&mut self) {
        if self.cursor==0 { return; }
        let mut s=self.cursor-1;
        while s>0 && (self.buf[s]&0xC0)==0x80 { s-=1; }
        self.buf.drain(s..self.cursor); self.cursor=s;
    }
    fn del_fwd(&mut self) {
        if self.cursor>=self.buf.len() { return; }
        let mut e=self.cursor+1;
        while e<self.buf.len() && (self.buf[e]&0xC0)==0x80 { e+=1; }
        self.buf.drain(self.cursor..e);
    }
    fn mv_left(&mut self) {
        if self.cursor==0 { return; }
        self.cursor-=1; while self.cursor>0 && (self.buf[self.cursor]&0xC0)==0x80 { self.cursor-=1; }
    }
    fn mv_right(&mut self) {
        if self.cursor>=self.buf.len() { return; }
        self.cursor+=1; while self.cursor<self.buf.len() && (self.buf[self.cursor]&0xC0)==0x80 { self.cursor+=1; }
    }
    fn mv_word_l(&mut self) {
        while self.cursor>0 && self.buf[self.cursor.saturating_sub(1)]==b' ' { self.cursor-=1; }
        while self.cursor>0 && self.buf[self.cursor.saturating_sub(1)]!=b' ' { self.cursor-=1; }
    }
    fn mv_word_r(&mut self) {
        while self.cursor<self.buf.len() && self.buf[self.cursor]==b' ' { self.cursor+=1; }
        while self.cursor<self.buf.len() && self.buf[self.cursor]!=b' ' { self.cursor+=1; }
    }
    fn mv_start(&mut self) { self.cursor=0; }
    fn mv_end(&mut self)   { self.cursor=self.buf.len(); }
    fn del_word_back(&mut self) {
        while self.cursor>0 && self.buf[self.cursor-1]==b' ' { self.del_back(); }
        while self.cursor>0 && self.buf[self.cursor-1]!=b' ' { self.del_back(); }
    }
    fn del_to_start(&mut self) { self.buf.drain(..self.cursor); self.cursor=0; }
    fn del_to_end(&mut self)   { self.buf.truncate(self.cursor); }
    fn set_buf(&mut self, s: &str) { self.buf=s.bytes().collect(); self.cursor=self.buf.len(); }

    fn hist_up(&mut self, hist: &History) {
        if hist.len()==0 { return; }
        let line=self.line();
        let idx = match self.hist_idx {
            None => { self.saved=self.buf.clone(); hist.len()-1 }
            Some(0) => 0,
            Some(i) => {
                // prefix search
                let pfx = if i<hist.len() { line.clone() } else { String::new() };
                hist.prefix_search_back(&pfx, i).unwrap_or(i.saturating_sub(1))
            }
        };
        if let Some(e)=hist.get(idx) { self.hist_idx=Some(idx); self.set_buf(e); }
    }
    fn hist_dn(&mut self, hist: &History) {
        match self.hist_idx {
            None => {}
            Some(i) if i+1>=hist.len() => { self.hist_idx=None; self.buf=self.saved.clone(); self.cursor=self.buf.len(); }
            Some(i) => { if let Some(e)=hist.get(i+1) { self.hist_idx=Some(i+1); self.set_buf(e); } }
        }
    }

    fn redraw(&self, plen: usize, sh: &Shell, hist: &History) {
        out("\r\x1b[K");
        let line=self.line();
        out(&highlight_line(&line, sh));
        // Ghost suggestion
        if self.cursor==self.buf.len() && !line.is_empty() {
            if let Some(sug)=hist.suggestion(&line) {
                let ghost=&sug[line.len()..];
                if !ghost.is_empty() {
                    out(C_DIM); out(C_GRAY); out(ghost); out(C_RST);
                    let gc=ghost.chars().count();
                    if gc>0 { out(&alloc::format!("\x1b[{}D",gc)); }
                }
            }
        }
        // Position cursor
        let lc=line.chars().count();
        let cc=String::from_utf8_lossy(&self.buf[..self.cursor]).chars().count();
        let diff=lc.saturating_sub(cc);
        if diff>0 { out(&alloc::format!("\x1b[{}D",diff)); }
    }

    fn draw_search(&self, hist: &History) {
        out("\r\x1b[K");
        out(C_CYN); out("(reverse-i-search)'"); out(C_RST);
        out(&self.srch_q); out(C_CYN); out("': "); out(C_RST);
        if let Some(idx)=self.srch_idx { if let Some(e)=hist.get(idx) { out(e); } }
    }

    fn accept_suggestion(&mut self, hist: &History) -> bool {
        let line=self.line();
        if self.cursor<self.buf.len() { self.cursor=self.buf.len(); return true; }
        if let Some(sug)=hist.suggestion(&line) { self.set_buf(&sug); return true; }
        false
    }

    fn read_line(&mut self, sh: &mut Shell, hist: &mut History, plen: usize) -> ReadResult {
        self.buf.clear(); self.cursor=0; self.hist_idx=None; self.saved.clear();
        self.srch_mode=false; self.srch_q.clear(); self.srch_idx=None;
        self.width=term_width();

        loop {
            let b = match read_byte() { Some(b)=>b, None=>return ReadResult::Eof };

            // ── Search mode ───────────────────────────────────────────
            if self.srch_mode {
                match b {
                    3|7 => { // Ctrl+C/G cancel
                        self.srch_mode=false; out("\r\x1b[K");
                        self.redraw(plen,sh,hist);
                    }
                    10|13 => { // Enter accept
                        if let Some(i)=self.srch_idx { if let Some(e)=hist.get(i){self.set_buf(e);} }
                        self.srch_mode=false; out("\r\x1b[K");
                        out(&highlight_line(&self.line(),sh)); out("\r\n");
                        let l=self.line(); if !l.trim().is_empty(){hist.push(&l);}
                        return ReadResult::Line(l);
                    }
                    18 => { // Ctrl+R search again
                        let from=self.srch_idx.unwrap_or(hist.len());
                        self.srch_idx=hist.search_back(&self.srch_q, from);
                        self.draw_search(hist);
                    }
                    127|8 => {
                        self.srch_q.pop();
                        self.srch_idx=hist.search_back(&self.srch_q,hist.len());
                        self.draw_search(hist);
                    }
                    c if c>=32 => {
                        self.srch_q.push(c as char);
                        self.srch_idx=hist.search_back(&self.srch_q,hist.len());
                        self.draw_search(hist);
                    }
                    _ => { self.srch_mode=false; self.redraw(plen,sh,hist); }
                }
                continue;
            }

            match b {
                // Enter
                10|13 => {
                    out("\r\n");
                    let line=self.line();
                    // Multiline continuation
                    let mut full=line.clone();
                    while full.ends_with('\\') {
                        full.pop();
                        out(C_BYEL); out("> "); out(C_RST);
                        self.buf=full.bytes().collect(); self.cursor=self.buf.len();
                        let cont = match read_byte() {
                            Some(10)|Some(13) => { out("\r\n"); String::new() }
                            Some(c) => {
                                let mut tmp_ed=Editor::new();
                                tmp_ed.insert_ch(c);
                                loop {
                                    match read_byte() {
                                        Some(10)|Some(13) => { out("\r\n"); break; }
                                        Some(127)|Some(8) => { tmp_ed.del_back(); tmp_ed.redraw(2,sh,hist); }
                                        Some(c) if c>=32  => { tmp_ed.insert_ch(c); tmp_ed.redraw(2,sh,hist); }
                                        _ => break,
                                    }
                                }
                                tmp_ed.line()
                            }
                            None => break,
                        };
                        full.push_str(&cont);
                    }
                    if !full.trim().is_empty() { hist.push(&full); }
                    return ReadResult::Line(full);
                }
                4  => { // Ctrl+D
                    if self.buf.is_empty() { out("\r\n"); return ReadResult::Eof; }
                    self.del_fwd(); self.redraw(plen,sh,hist);
                }
                3  => { // Ctrl+C
                    out(C_BRED); out("^C"); out(C_RST); out("\r\n");
                    self.buf.clear(); self.cursor=0; return ReadResult::Line(String::new());
                }
                12 => { out("\x1b[2J\x1b[H"); self.redraw(plen,sh,hist); } // Ctrl+L
                18 => { // Ctrl+R
                    self.srch_mode=true; self.srch_q.clear(); self.srch_idx=None;
                    self.draw_search(hist);
                }
                1  => { self.mv_start(); self.redraw(plen,sh,hist); } // Ctrl+A
                5  => { self.mv_end();   self.redraw(plen,sh,hist); } // Ctrl+E
                6  => { self.mv_right(); self.redraw(plen,sh,hist); } // Ctrl+F
                2  => { self.mv_left();  self.redraw(plen,sh,hist); } // Ctrl+B
                16 => { self.hist_up(hist); self.redraw(plen,sh,hist); } // Ctrl+P
                14 => { self.hist_dn(hist); self.redraw(plen,sh,hist); } // Ctrl+N
                23 => { self.del_word_back(); self.redraw(plen,sh,hist); } // Ctrl+W
                21 => { self.del_to_start(); self.redraw(plen,sh,hist); }  // Ctrl+U
                11 => { self.del_to_end();   self.redraw(plen,sh,hist); }  // Ctrl+K
                9  => { // Tab
                    let line=self.line();
                    let comps=complete_at(&line, self.cursor, sh);
                    if comps.is_empty() { continue; }
                    if comps.len()==1 {
                        let ws=line.rfind(|c:char|c==' '||c=='|'||c==';'||c=='('||c=='&').map(|i|i+1).unwrap_or(0);
                        let cur_word=&line[ws..self.cursor];
                        let suffix=&comps[0].insert[cur_word.len()..];
                        self.insert_str(suffix);
                        if !comps[0].insert.ends_with('/') { self.insert_ch(b' '); }
                        self.redraw(plen,sh,hist);
                    } else {
                        out("\r\n");
                        let w=self.width.max(20);
                        let cw=comps.iter().map(|c|c.display.len()).max().unwrap_or(0)+2;
                        let cols=(w/cw).max(1);
                        for (i,c) in comps.iter().enumerate() {
                            let color=match c.kind{CompKind::Dir=>C_BBLU,CompKind::Cmd=>C_BGRN,CompKind::Builtin=>C_CYN,CompKind::Alias=>C_YEL,CompKind::Pid=>C_MAG,_=>"\x1b[0m"};
                            out(color); out(&c.display); out(C_RST);
                            for _ in 0..cw.saturating_sub(c.display.len()) { out(" "); }
                            if (i+1)%cols==0 { out("\r\n"); }
                        }
                        if comps.len()%cols!=0 { out("\r\n"); }
                        // Complete common prefix
                        let pfx=comp_prefix(&comps);
                        let ws=line.rfind(|c:char|c==' '||c=='|'||c==';'||c=='('||c=='&').map(|i|i+1).unwrap_or(0);
                        let cur=&line[ws..self.cursor];
                        if pfx.len()>cur.len() { self.insert_str(&pfx[cur.len()..]); }
                        self.redraw(plen,sh,hist);
                    }
                }
                127|8 => { self.del_back(); self.redraw(plen,sh,hist); } // Backspace
                27 => { // ESC
                    let mut esc=[0u8;8]; let en=read_more_bytes(&mut esc);
                    if en>0 { self.handle_esc(&esc[..en as usize], hist, sh, plen); }
                }
                c if c>=32 => { self.insert_ch(c); self.redraw(plen,sh,hist); }
                _ => {}
            }
        }
    }

    fn handle_esc(&mut self, seq: &[u8], hist: &History, sh: &Shell, plen: usize) {
        if seq.is_empty() { return; }
        if seq[0]==b'[' && seq.len()>=2 {
            match seq[1] {
                b'A' => { self.hist_up(hist); self.redraw(plen,sh,hist); }
                b'B' => { self.hist_dn(hist); self.redraw(plen,sh,hist); }
                b'C' => { // Right arrow or accept suggestion
                    if !self.accept_suggestion(hist) { self.mv_right(); }
                    self.redraw(plen,sh,hist);
                }
                b'D' => { self.mv_left();  self.redraw(plen,sh,hist); }
                b'H' => { self.mv_start(); self.redraw(plen,sh,hist); }
                b'F' => { self.mv_end();   self.redraw(plen,sh,hist); }
                b'3' if seq.len()>=3 && seq[2]==b'~' => { self.del_fwd(); self.redraw(plen,sh,hist); }
                b'1' if seq.len()>=5 && seq[2]==b';' && seq[3]==b'5' => {
                    match seq[4] { b'C'=>{ self.mv_word_r(); self.redraw(plen,sh,hist); } b'D'=>{ self.mv_word_l(); self.redraw(plen,sh,hist); } _=>{} }
                }
                _ => {}
            }
        } else if seq[0]==b'O' && seq.len()>=2 {
            match seq[1] { b'H'=>{self.mv_start();self.redraw(plen,sh,hist);} b'F'=>{self.mv_end();self.redraw(plen,sh,hist);} _=>{} }
        } else {
            // Alt+key
            match seq[0] {
                b'b'|b'B' => { self.mv_word_l(); self.redraw(plen,sh,hist); }
                b'f'|b'F' => { self.mv_word_r(); self.redraw(plen,sh,hist); }
                b'd'|b'D' => {
                    let s=self.cursor; self.mv_word_r(); self.buf.drain(s..self.cursor); self.cursor=s;
                    self.redraw(plen,sh,hist);
                }
                127 => { self.del_word_back(); self.redraw(plen,sh,hist); }
                _ => {}
            }
        }
    }
}

// ── Prompt ────────────────────────────────────────────────────────────────
fn hostname_short() -> String {
    let mut buf=[0u8;64]; let fd=open(b"/etc/hostname\0",O_RDONLY,0);
    if fd>=0 { let n=read(fd as i32,&mut buf); close(fd as i32);
        if n>0 { return String::from_utf8_lossy(&buf[..n as usize]).trim().split('.').next().unwrap_or("qunix").to_string(); } }
    "qunix".to_string()
}

fn git_branch(cwd: &str) -> String {
    let mut path=cwd.to_string();
    for _ in 0..8 {
        let hp=alloc::format!("{}/.git/HEAD\0",path);
        let fd=open(hp.as_bytes(),O_RDONLY,0);
        if fd>=0 {
            let mut buf=[0u8;256]; let n=read(fd as i32,&mut buf); close(fd as i32);
            if n>0 {
                let s=String::from_utf8_lossy(&buf[..n as usize]); let s=s.trim();
                if let Some(b)=s.strip_prefix("ref: refs/heads/") {
                    return alloc::format!(" {}({}){} ",C_MAG,b.trim(),C_RST);
                } else if s.len()>=7 { return alloc::format!(" {}({}){} ",C_MAG,&s[..7],C_RST); }
            }
        }
        if path=="/" { break; }
        match path.rfind('/') { Some(0)=>path="/".to_string(), Some(p)=>path=path[..p].to_string(), None=>break }
    }
    String::new()
}

fn render_prompt(sh: &Shell, last_status: i32) -> String {
    let ps1=sh.env.get("PS1").cloned().unwrap_or_else(||
        "\\[\x1b[1;32m\\]\\u@\\h\\[\x1b[0m\\]:\\[\x1b[1;34m\\]\\w\\[\x1b[0m\\]\\g\\$ ".to_string());
    expand_ps1(&ps1, sh, last_status)
}

fn expand_ps1(ps1: &str, sh: &Shell, last_status: i32) -> String {
    let mut o=String::new(); let mut chars=ps1.chars().peekable();
    while let Some(c)=chars.next() {
        if c=='\\' { match chars.next() {
            Some('u') => o.push_str(&sh.env.get("USER").cloned().unwrap_or_else(||"user".to_string())),
            Some('h') => o.push_str(&hostname_short()),
            Some('H') => o.push_str(&hostname_short()),
            Some('w') => {
                let home=sh.env.get("HOME").cloned().unwrap_or_else(||"/".to_string());
                let cwd=&sh.cwd;
                if cwd.starts_with(&home) { o.push('~'); o.push_str(&cwd[home.len()..]); }
                else { o.push_str(cwd); }
            }
            Some('W') => { let cwd=&sh.cwd; o.push_str(cwd.rfind('/').map(|i|&cwd[i+1..]).unwrap_or(cwd).if_empty("/")); }
            Some('$') => o.push(if getuid()==0{'#'}else{'$'}),
            Some('n') => o.push('\n'),
            Some('t') => { let mut ts=[0i64;2]; clock_gettime(0,&mut ts); let s=ts[0]%86400; o.push_str(&alloc::format!("{:02}:{:02}:{:02}",s/3600,(s%3600)/60,s%60)); }
            Some('j') => o.push_str(&sh.jobs.len().to_string()),
            Some('?') => {
                if last_status==0 { o.push_str(&alloc::format!("{}0{}",C_BGRN,C_RST)); }
                else { o.push_str(&alloc::format!("{}{}{}",C_BRED,last_status,C_RST)); }
            }
            Some('g') => o.push_str(&git_branch(&sh.cwd)),
            Some('x') => {
                if last_status==0 { o.push_str(&alloc::format!("{}✓{}",C_BGRN,C_RST)); }
                else { o.push_str(&alloc::format!("{}✗{}",C_BRED,C_RST)); }
            }
            Some('s') => o.push_str("qsh"),
            Some('v') => o.push_str("2.0"),
            Some('[') | Some(']') => {}  // readline non-printing markers
            Some('e') | Some('E') => o.push('\x1b'),
            Some('a') => o.push('\x07'),
            Some('\\') => o.push('\\'),
            Some(c) => { o.push('\\'); o.push(c); }
            None => {}
        }} else if c=='\x01'||c=='\x02' {
            // ignore readline markers
        } else { o.push(c); }
    }
    o
}

trait StrExt { fn if_empty<'a>(&'a self, default: &'a str) -> &'a str; }
impl StrExt for str { fn if_empty<'a>(&'a self, default: &'a str) -> &'a str { if self.is_empty(){default}else{self} } }

fn prompt_visible_len(s: &str) -> usize {
    let mut n=0; let mut in_esc=false;
    for c in s.chars() {
        if c=='\x1b'{in_esc=true;continue;}
        if in_esc{if c.is_alphabetic()||c=='m'{in_esc=false;}continue;}
        if c=='\x01'||c=='\x02'{continue;}
        n+=1;
    }
    n
}

// ── Glob / brace expansion ────────────────────────────────────────────────
fn glob_expand(pattern: &str) -> Vec<String> {
    if !pattern.contains('*') && !pattern.contains('?') && !pattern.contains('[') {
        return alloc::vec![pattern.to_string()];
    }
    let mut results=Vec::new();
    if pattern.contains("**") { glob_recurse(pattern,&mut results); }
    else { glob_dir(pattern,&mut results); }
    if results.is_empty() { alloc::vec![pattern.to_string()] } else { results.sort(); results.dedup(); results }
}

fn glob_dir(pattern: &str, results: &mut Vec<String>) {
    let (dir,file_pat)=if let Some(s)=pattern.rfind('/') { (&pattern[..s],&pattern[s+1..]) } else { (".",pattern) };
    let dp=alloc::format!("{}\0",dir);
    let fd=open(dp.as_bytes(),O_RDONLY|O_DIRECTORY,0); if fd<0{return;}
    let mut buf=alloc::vec![0u8;32768];
    loop {
        let n=getdents64(fd as i32,&mut buf); if n<=0{break;}
        let mut off=0;
        while off<n as usize {
            let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
            let nb=&buf[off+19..]; let nl=nb.iter().position(|&b|b==0).unwrap_or(0);
            let name=String::from_utf8_lossy(&nb[..nl]).to_string();
            if (!name.starts_with('.')||file_pat.starts_with('.')) && glob_match(&name,file_pat) {
                let full=if dir=="." {name.clone()} else {alloc::format!("{}/{}",dir,name)};
                results.push(full);
            }
            if reclen==0{break;} off+=reclen;
        }
    }
    close(fd as i32);
}

fn glob_recurse(pattern: &str, results: &mut Vec<String>) {
    let parts: Vec<&str>=pattern.splitn(2,"**").collect();
    if parts.len()<2 { glob_dir(pattern,results); return; }
    let prefix=parts[0].trim_end_matches('/');
    let suffix=parts[1].trim_start_matches('/');
    let root=if prefix.is_empty(){"."} else{prefix};
    glob_walk(root,suffix,results);
}

fn glob_walk(dir: &str, suffix: &str, results: &mut Vec<String>) {
    if !suffix.is_empty() {
        let cand=alloc::format!("{}/{}",dir,suffix); glob_dir(&cand,results);
    } else { results.push(dir.to_string()); }
    let dp=alloc::format!("{}\0",dir);
    let fd=open(dp.as_bytes(),O_RDONLY|O_DIRECTORY,0); if fd<0{return;}
    let mut buf=alloc::vec![0u8;32768]; let mut subs=Vec::new();
    loop {
        let n=getdents64(fd as i32,&mut buf); if n<=0{break;}
        let mut off=0;
        while off<n as usize {
            let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
            let dtype=buf[off+18];
            let nb=&buf[off+19..]; let nl=nb.iter().position(|&b|b==0).unwrap_or(0);
            let name=String::from_utf8_lossy(&nb[..nl]).to_string();
            if dtype==4 && name!="." && name!=".." && !name.starts_with('.') { subs.push(name); }
            if reclen==0{break;} off+=reclen;
        }
    }
    close(fd as i32);
    for s in subs { glob_walk(&alloc::format!("{}/{}",dir,s),suffix,results); }
}

fn glob_match(name: &str, pat: &str) -> bool {
    glob_inner(&pat.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}
fn glob_inner(p: &[char], n: &[char]) -> bool {
    if p.is_empty() { return n.is_empty(); }
    match p[0] {
        '*' => (0..=n.len()).any(|i| glob_inner(&p[1..],&n[i..])),
        '?' => !n.is_empty() && glob_inner(&p[1..],&n[1..]),
        '[' => {
            if n.is_empty(){return false;}
            let (ok,len)=bracket_match(&p[1..],n[0]);
            ok && glob_inner(&p[1+len..],&n[1..])
        }
        c => !n.is_empty() && c==n[0] && glob_inner(&p[1..],&n[1..]),
    }
}
fn bracket_match(p: &[char], c: char) -> (bool,usize) {
    let neg=p.first()==Some(&'!')||p.first()==Some(&'^');
    let st=if neg{1}else{0}; let mut i=st; let mut ok=false;
    while i<p.len()&&p[i]!=']' {
        if i+2<p.len()&&p[i+1]=='-'&&p[i+2]!=']' { if c>=p[i]&&c<=p[i+2]{ok=true;} i+=3; }
        else { if c==p[i]{ok=true;} i+=1; }
    }
    (if neg{!ok}else{ok}, if i<p.len(){i+1}else{i})
}

fn brace_expand(s: &str) -> Vec<String> {
    if !s.contains('{') { return alloc::vec![s.to_string()]; }
    let bytes=s.as_bytes(); let mut bst=None; let mut d=0i32;
    for (i,&b) in bytes.iter().enumerate() { if b==b'{'{if d==0{bst=Some(i);}d+=1;}else if b==b'}'{d-=1;if d==0{break;}} }
    let bst=match bst{Some(i)=>i,None=>return alloc::vec![s.to_string()]};
    let mut bend=bst; d=0;
    for i in bst..bytes.len(){if bytes[i]==b'{'{d+=1;}else if bytes[i]==b'}'{d-=1;if d==0{bend=i;break;}}}
    let prefix=&s[..bst]; let inner=&s[bst+1..bend]; let suffix=&s[bend+1..];
    let alts=brace_alts(inner);
    let mut res=Vec::new();
    for alt in &alts {
        for p in brace_expand(prefix) { for a in brace_expand(alt) { for sx in brace_expand(suffix) {
            res.push(alloc::format!("{}{}{}",p,a,sx));
        }}}
    }
    if res.is_empty(){alloc::vec![s.to_string()]}else{res}
}
fn brace_alts(inner: &str) -> Vec<String> {
    if let Some(p)=inner.find("..") {
        let s=&inner[..p]; let rest=&inner[p+2..];
        let (e,step)=if let Some(sp)=rest.find(".."){(&rest[..sp],rest[sp+2..].parse::<i64>().unwrap_or(1))}else{(rest,1)};
        let step=if step==0{1}else{step.abs()};
        if let (Ok(a),Ok(b))=(s.parse::<i64>(),e.parse::<i64>()) {
            let mut v=Vec::new();
            if a<=b{let mut i=a;while i<=b{v.push(i.to_string());i+=step;}}
            else   {let mut i=a;while i>=b{v.push(i.to_string());i-=step;}}
            return v;
        }
        if s.len()==1&&e.len()==1 {
            let sc=s.chars().next().unwrap(); let ec=e.chars().next().unwrap();
            let mut v=Vec::new();
            if sc<=ec{for c in sc..=ec{v.push(c.to_string());}}
            else{let mut c=sc;loop{v.push(c.to_string());if c==ec{break;}if let Some(nc)=char::from_u32(c as u32-1){c=nc;}else{break;}}}
            return v;
        }
    }
    let mut alts=Vec::new(); let mut d=0i32; let mut st=0;
    for (i,c) in inner.chars().enumerate() {
        if c=='{'{d+=1;}else if c=='}'{d-=1;}
        else if c==','&&d==0{alts.push(inner[st..i].to_string());st=i+1;}
    }
    alts.push(inner[st..].to_string()); alts
}

// ── Tokenizer / expansion / arith ────────────────────────────────────────
#[derive(Debug,Clone,PartialEq)]
enum Tok { Word(String), Assign(String,String), Pipe, PipeErr, And, Or, Semi, Amp, Nl,
    Rout(Option<u8>,String,bool), Rin(Option<u8>,String), RHere(String), RHereDoc(String,String), RDup(u8,u8),
    LP, RP, LB, RB, Bang, If, Then, Elif, Else, Fi, While, Until, Do, Done, For, In,
    Case, Esac, Func, Fn, Sub(String), CmdSub(String), ArithSub(String) }

fn tokenize(input: &str) -> Vec<Tok> {
    let mut toks=Vec::new(); let chars: Vec<char>=input.chars().collect();
    let mut i=0; let n=chars.len();
    while i<n {
        match chars[i] {
            ' '|'\t' => { i+=1; }
            '\n' => { toks.push(Tok::Nl); i+=1; }
            '#' => { while i<n&&chars[i]!='\n'{i+=1;} }
            '|' => { i+=1;
                if i<n&&chars[i]=='|'{i+=1;toks.push(Tok::Or);}
                else if i<n&&chars[i]=='&'{i+=1;toks.push(Tok::PipeErr);}
                else{toks.push(Tok::Pipe);}
            }
            '&' => { i+=1;
                if i<n&&chars[i]=='&'{i+=1;toks.push(Tok::And);}
                else if i<n&&chars[i]=='>'{ i+=1; let app=i<n&&chars[i]=='>'; if app{i+=1;}
                    let f=skip_read(&chars,&mut i); toks.push(Tok::Rout(None,f,app)); }
                else{toks.push(Tok::Amp);}
            }
            ';' => { i+=1; if i<n&&chars[i]==';'{i+=1;} toks.push(Tok::Semi); }
            '!' => { i+=1; toks.push(Tok::Bang); }
            '(' => { i+=1; let s=balanced(&chars,&mut i,')',  '('); toks.push(Tok::Sub(s)); }
            ')' => { i+=1; toks.push(Tok::RP); }
            '{' => { i+=1; toks.push(Tok::LB); }
            '}' => { i+=1; toks.push(Tok::RB); }
            '<' => { i+=1;
                if i<n&&chars[i]=='<'{i+=1;
                    if i<n&&chars[i]=='<'{i+=1; let s=skip_read(&chars,&mut i); toks.push(Tok::RHere(s));}
                    else { let d=skip_read(&chars,&mut i); let c=heredoc(&chars,&mut i,&d); toks.push(Tok::RHereDoc(d,c)); }
                } else { let f=skip_read(&chars,&mut i); toks.push(Tok::Rin(None,f)); }
            }
            '>' => { i+=1; let app=i<n&&chars[i]=='>'; if app{i+=1;}
                if i<n&&chars[i]=='&'{ i+=1; let d=skip_read(&chars,&mut i); toks.push(Tok::RDup(1,d.parse().unwrap_or(1))); }
                else { let f=skip_read(&chars,&mut i); toks.push(Tok::Rout(None,f,app)); }
            }
            '0'..='9' => {
                let st=i; while i<n&&chars[i].is_ascii_digit(){i+=1;}
                let num: String=chars[st..i].iter().collect();
                if i<n&&chars[i]=='>'{ i+=1; let app=i<n&&chars[i]=='>'; if app{i+=1;}
                    if i<n&&chars[i]=='&'{ i+=1; let d=skip_read(&chars,&mut i); toks.push(Tok::RDup(num.parse().unwrap_or(1),d.parse().unwrap_or(1))); }
                    else { let f=skip_read(&chars,&mut i); toks.push(Tok::Rout(num.parse().ok(),f,app)); }
                } else if i<n&&chars[i]=='<'{ i+=1; let f=skip_read(&chars,&mut i); toks.push(Tok::Rin(num.parse().ok(),f)); }
                else { i=st; let w=word(&chars,&mut i); toks.push(cls(w)); }
            }
            _ => { let w=word(&chars,&mut i); toks.push(cls(w)); }
        }
    }
    toks
}
fn cls(w: String) -> Tok {
    match w.as_str() {
        "if"=>Tok::If,"then"=>Tok::Then,"elif"=>Tok::Elif,"else"=>Tok::Else,"fi"=>Tok::Fi,
        "while"=>Tok::While,"until"=>Tok::Until,"do"=>Tok::Do,"done"=>Tok::Done,
        "for"=>Tok::For,"in"=>Tok::In,"case"=>Tok::Case,"esac"=>Tok::Esac,
        "function"=>Tok::Func,"fn"=>Tok::Fn,"{" =>Tok::LB,"}"|"}"=>Tok::RB,"!"=>Tok::Bang,
        _ => {
            if let Some(eq)=w.find('=') {
                let name=&w[..eq];
                if is_varname(name) { return Tok::Assign(name.to_string(),w[eq+1..].to_string()); }
            }
            Tok::Word(w)
        }
    }
}
fn is_varname(s: &str) -> bool {
    let mut c=s.chars(); match c.next(){Some(x) if x.is_alphabetic()||x=='_'=>{} _=>return false}
    c.all(|x|x.is_alphanumeric()||x=='_')
}
fn word(chars: &[char], i: &mut usize) -> String {
    let mut w=String::new(); let n=chars.len();
    while *i<n {
        match chars[*i] {
            ' '|'\t'|'\n'|';'|'&'|'|'|'('|')'|'{'|'}'|'<'|'>'|'#' => break,
            '\'' => { *i+=1; while *i<n&&chars[*i]!='\''{w.push(chars[*i]);*i+=1;} if *i<n{*i+=1;} }
            '"'  => { *i+=1;
                while *i<n&&chars[*i]!='"' {
                    if chars[*i]=='\\'&&*i+1<n { *i+=1;
                        match chars[*i]{'\"'|'\\'|'$'|'`'|'\n'=>w.push(chars[*i]),'n'=>w.push('\n'),'t'=>w.push('\t'),'r'=>w.push('\r'),'a'=>w.push('\x07'),c=>{w.push('\\');w.push(c);}}
                    } else { w.push(chars[*i]); }
                    *i+=1;
                }
                if *i<n{*i+=1;}
            }
            '\\' => { *i+=1; if *i<n{w.push(chars[*i]);*i+=1;} }
            '$'  => { *i+=1; if *i<n {
                match chars[*i] {
                    '(' => { *i+=1;
                        if *i<n&&chars[*i]=='('{ *i+=1;
                            let expr=balanced(chars,i,')','('); if *i<n{*i+=1;}
                            w.push_str(&alloc::format!("$(({}))",expr));
                        } else { let s=balanced(chars,i,')','('); w.push_str(&alloc::format!("$({})",s)); }
                    }
                    '{' => { *i+=1; let v=balanced(chars,i,'}','{'); w.push_str(&alloc::format!("${{{}}}",v)); }
                    c if c.is_alphanumeric()||c=='_'||"?$!@*#0".contains(c) => {
                        w.push('$'); w.push(chars[*i]); *i+=1;
                        if c.is_alphanumeric()||c=='_' { while *i<n&&(chars[*i].is_alphanumeric()||chars[*i]=='_'){w.push(chars[*i]);*i+=1;} }
                        continue;
                    }
                    _ => w.push('$'),
                }
            } else { w.push('$'); } continue; }
            '`' => { *i+=1; let mut s=String::new(); while *i<n&&chars[*i]!='`'{s.push(chars[*i]);*i+=1;} if *i<n{*i+=1;} w.push_str(&alloc::format!("`{}`",s)); }
            c => { w.push(c); *i+=1; }
        }
    }
    w
}
fn skip_read(chars: &[char], i: &mut usize) -> String {
    while *i<chars.len()&&(chars[*i]==' '||chars[*i]=='\t'){*i+=1;} word(chars,i)
}
fn balanced(chars: &[char], i: &mut usize, close: char, open: char) -> String {
    let mut d=1i32; let mut s=String::new(); let n=chars.len();
    while *i<n&&d>0 {
        if chars[*i]==open{d+=1;} if chars[*i]==close{d-=1;if d==0{*i+=1;break;}}
        s.push(chars[*i]); *i+=1;
    }
    s
}
fn heredoc(chars: &[char], i: &mut usize, delim: &str) -> String {
    while *i<chars.len()&&chars[*i]!='\n'{*i+=1;} if *i<chars.len(){*i+=1;}
    let mut content=String::new(); let mut line=String::new(); let n=chars.len();
    while *i<n {
        if chars[*i]=='\n' {
            if line.trim()==delim{*i+=1;break;}
            content.push_str(&line); content.push('\n'); line.clear();
        } else { line.push(chars[*i]); }
        *i+=1;
    }
    content
}

fn expand(s: &str, sh: &Shell) -> String {
    if !s.contains('$')&&!s.contains('`')&&!s.contains('~'){return s.to_string();}
    let mut res=String::new(); let chars: Vec<char>=s.chars().collect(); let mut i=0;
    while i<chars.len() {
        match chars[i] {
            '~' if i==0 => { res.push_str(&sh.env.get("HOME").cloned().unwrap_or_else(||"/root".to_string())); i+=1; }
            '$' => { i+=1; if i>=chars.len(){res.push('$');break;}
                match chars[i] {
                    '{' => { i+=1; let mut v=String::new(); let mut d=1i32;
                        while i<chars.len(){ if chars[i]=='{'{d+=1;} if chars[i]=='}'{d-=1;if d==0{i+=1;break;}} v.push(chars[i]);i+=1; }
                        res.push_str(&expand_brace_var(&v,sh)); }
                    '(' => { i+=1;
                        if i<chars.len()&&chars[i]=='('{ i+=1; let mut e=String::new(); let mut d=2i32;
                            while i<chars.len(){if chars[i]=='('{d+=1;}if chars[i]==')'{d-=1;if d==0{i+=1;break;}}e.push(chars[i]);i+=1;}
                            res.push_str(&eval_arith(&expand(&e,sh)).to_string());
                        } else { let mut c=String::new(); let mut d=1i32;
                            while i<chars.len(){if chars[i]=='('{d+=1;}if chars[i]==')'{d-=1;if d==0{i+=1;break;}}c.push(chars[i]);i+=1;}
                            res.push_str(&cmd_sub(&c,sh)); }
                    }
                    '?' => { res.push_str(&sh.last_status.to_string()); i+=1; }
                    '$' => { res.push_str(&sh.pid.to_string()); i+=1; }
                    '!' => { res.push_str("0"); i+=1; }
                    '#' => { res.push('0'); i+=1; }
                    '@'|'*' => { res.push(' '); i+=1; }
                    '0' => { res.push_str("qsh"); i+=1; }
                    _ => { let mut v=String::new();
                        while i<chars.len()&&(chars[i].is_alphanumeric()||chars[i]=='_'){v.push(chars[i]);i+=1;}
                        if let Some(val)=sh.env.get(&v){res.push_str(val);}
                        else if sh.opt_nounset{err(&alloc::format!("qsh: {}: unbound variable\n",v));}
                    }
                }
            }
            '`' => { i+=1; let mut c=String::new(); while i<chars.len()&&chars[i]!='`'{c.push(chars[i]);i+=1;} if i<chars.len(){i+=1;} res.push_str(&cmd_sub(&c,sh)); }
            c => { res.push(c); i+=1; }
        }
    }
    res
}

fn expand_brace_var(spec: &str, sh: &Shell) -> String {
    if spec.starts_with('#') { return sh.env.get(&spec[1..]).map(|v|v.len().to_string()).unwrap_or_else(||"0".to_string()); }
    for (op,l) in [(":-",2usize),(":=",2),(":+",2),(":?",2)] {
        if let Some(p)=spec.find(op) {
            let name=&spec[..p]; let val=spec[p+l..].to_string();
            let cur=sh.env.get(name).cloned();
            return match op {
                ":-" => cur.filter(|v|!v.is_empty()).unwrap_or_else(||expand(&val,sh)),
                ":=" => { if cur.as_deref().map(|v|v.is_empty()).unwrap_or(true){expand(&val,sh)}else{cur.unwrap_or_default()} }
                ":+" => { if cur.filter(|v|!v.is_empty()).is_some(){expand(&val,sh)}else{String::new()} }
                ":?" => { cur.filter(|v|!v.is_empty()).unwrap_or_else(||{err(&alloc::format!("qsh: {}: {}",name,expand(&val,sh)));String::new()}) }
                _ => String::new(),
            };
        }
    }
    if let Some(p)=spec.find('#') { if p>0 { return sh.env.get(&spec[..p]).map(|v|v.trim_start_matches(&spec[p+1..]).to_string()).unwrap_or_default(); } }
    if let Some(p)=spec.find('%') { return sh.env.get(&spec[..p]).map(|v|v.trim_end_matches(&spec[p+1..]).to_string()).unwrap_or_default(); }
    if let Some(p)=spec.find('/') {
        let name=&spec[..p]; let rest=&spec[p+1..];
        if let Some(sp)=rest.find('/') { let pat=&rest[..sp]; let rep=&rest[sp+1..];
            if let Some(v)=sh.env.get(name){return v.replacen(pat,rep,1);} }
        return String::new();
    }
    sh.env.get(spec).cloned().unwrap_or_default()
}

fn cmd_sub(cmd: &str, sh: &Shell) -> String {
    let mut fds=[0i32;2]; pipe(&mut fds);
    let pid=fork();
    if pid==0 { close(fds[0]); dup2(fds[1],STDOUT); close(fds[1]);
        let mut sh2=Shell::new_child(sh); sh2.exec_string(cmd); exit(0); }
    close(fds[1]);
    let mut out_buf=alloc::vec![0u8;65536]; let mut tot=0;
    loop { let n=read(fds[0],&mut out_buf[tot..]); if n<=0{break;} tot+=n as usize; }
    close(fds[0]);
    if pid>0 { waitpid(pid as i32,core::ptr::null_mut(),0); }
    while tot>0&&out_buf[tot-1]==b'\n'{tot-=1;}
    String::from_utf8_lossy(&out_buf[..tot]).to_string()
}

fn eval_arith(expr: &str) -> i64 { arith(expr.trim()) }
fn arith(e: &str) -> i64 {
    let e=e.trim(); if e.is_empty(){return 0;}
    if let Some(p)=fop(e,"||"){return (arith(&e[..p])!=0||arith(&e[p+2..])!=0) as i64;}
    if let Some(p)=fop(e,"&&"){return (arith(&e[..p])!=0&&arith(&e[p+2..])!=0) as i64;}
    if let Some(p)=fop(e,"|") {return arith(&e[..p])|arith(&e[p+1..]);}
    if let Some(p)=fop(e,"^") {return arith(&e[..p])^arith(&e[p+1..]);}
    if let Some(p)=fop(e,"&") {return arith(&e[..p])&arith(&e[p+1..]);}
    if let Some(p)=fop(e,"=="){return (arith(&e[..p])==arith(&e[p+2..])) as i64;}
    if let Some(p)=fop(e,"!="){return (arith(&e[..p])!=arith(&e[p+2..])) as i64;}
    if let Some(p)=fop(e,"<="){return (arith(&e[..p])<=arith(&e[p+2..])) as i64;}
    if let Some(p)=fop(e,">="){return (arith(&e[..p])>=arith(&e[p+2..])) as i64;}
    if let Some(p)=fop(e,"<") {return (arith(&e[..p])< arith(&e[p+1..])) as i64;}
    if let Some(p)=fop(e,">") {return (arith(&e[..p])> arith(&e[p+1..])) as i64;}
    if let Some(p)=fop(e,"<<"){ let s=arith(&e[p+2..]) as u32&63; return arith(&e[..p])<<s; }
    if let Some(p)=fop(e,">>"){ let s=arith(&e[p+2..]) as u32&63; return arith(&e[..p])>>s; }
    if let Some(p)=fop_pm(e,'+'){return arith(&e[..p])+arith(&e[p+1..]);}
    if let Some(p)=fop_pm(e,'-'){return arith(&e[..p])-arith(&e[p+1..]);}
    if let Some(p)=fop_pm(e,'*'){return arith(&e[..p])*arith(&e[p+1..]);}
    if let Some(p)=fop_pm(e,'/'){let d=arith(&e[p+1..]);return if d==0{0}else{arith(&e[..p])/d};}
    if let Some(p)=fop_pm(e,'%'){let d=arith(&e[p+1..]);return if d==0{0}else{arith(&e[..p])%d};}
    if e.starts_with('-'){return -arith(&e[1..]);}
    if e.starts_with('!'){return (arith(&e[1..])==0) as i64;}
    if e.starts_with('~'){return !arith(&e[1..]);}
    if e.starts_with('(')&&e.ends_with(')'){return arith(&e[1..e.len()-1]);}
    if let Some(s)=e.strip_prefix("0x").or_else(||e.strip_prefix("0X")){return i64::from_str_radix(s,16).unwrap_or(0);}
    e.parse::<i64>().unwrap_or(0)
}
fn fop(e: &str, op: &str) -> Option<usize> {
    let b=e.as_bytes(); let n=b.len(); let ol=op.len(); let mut d=0i32; let mut i=n;
    while i>ol{i-=1; match b[i]{b')'=>d+=1,b'('=>d-=1,_=>{}} if d==0&&i+ol<=n&&&b[i..i+ol]==op.as_bytes(){return Some(i);}}
    None
}
fn fop_pm(e: &str, op: char) -> Option<usize> {
    let b=e.as_bytes(); let mut d=0i32; let mut best=None; let mut i=b.len();
    while i>0{i-=1; match b[i]{b')'=>d+=1,b'('=>d-=1,_=>{}}
        if d==0&&b[i]==op as u8{if i>0&&b[i-1]==op as u8{continue;}if i+1<b.len()&&b[i+1]==op as u8{continue;} best=Some(i);break;}}
    best
}

fn eval_test(args: &[String], sh: &Shell) -> bool {
    if args.is_empty(){return false;}
    if args.len()>=2&&args[0]=="!"{return !eval_test(&args[1..],sh);}
    if let Some(p)=args.iter().position(|s|s=="-a"){return eval_test(&args[..p],sh)&&eval_test(&args[p+1..],sh);}
    if let Some(p)=args.iter().position(|s|s=="-o"){return eval_test(&args[..p],sh)||eval_test(&args[p+1..],sh);}
    if args.len()==2 {
        let op=&args[0]; let v=sh.expand(&args[1]);
        return match op.as_str() {
            "-n"=>!v.is_empty(),"-z"=>v.is_empty(),
            "-f"=>ftype(&v)=='f',"-d"=>ftype(&v)=='d',"-e"=>fexists(&v),
            "-r"=>fmode(&v,0o444),"-w"=>fmode(&v,0o222),"-x"=>fmode(&v,0o111),
            "-s"=>fsize(&v)>0,"-L"|"-h"=>ftype(&v)=='l',
            "-b"=>ftype(&v)=='b',"-c"=>ftype(&v)=='c',"-p"=>ftype(&v)=='p',
            "-t"=>v.parse::<i32>().map(|fd|isatty(fd)>0).unwrap_or(false),
            _=>!v.is_empty(),
        };
    }
    if args.len()==3 {
        let a=sh.expand(&args[0]); let op=&args[1]; let b=sh.expand(&args[2]);
        return match op.as_str() {
            "="|"=="|"-eq"=>a==b||(a.parse::<i64>().ok()==b.parse::<i64>().ok()),
            "!="|"-ne"=>a!=b,
            "<"|"-lt"=>a.parse::<i64>().unwrap_or(0)<b.parse::<i64>().unwrap_or(0),
            ">"|"-gt"=>a.parse::<i64>().unwrap_or(0)>b.parse::<i64>().unwrap_or(0),
            "-le"=>a.parse::<i64>().unwrap_or(0)<=b.parse::<i64>().unwrap_or(0),
            "-ge"=>a.parse::<i64>().unwrap_or(0)>=b.parse::<i64>().unwrap_or(0),
            "-nt"=>fmtime(&a)>fmtime(&b),"-ot"=>fmtime(&a)<fmtime(&b),"-ef"=>a==b,
            "=~"=>glob_match(&a,&b),_=>false,
        };
    }
    !args[0].is_empty()
}
fn fexists(p: &str)->bool{let mut st=Stat::default();let mut pp=p.to_string();pp.push('\0');stat(pp.as_bytes(),&mut st)>=0}
fn ftype(p: &str)->char{let mut st=Stat::default();let mut pp=p.to_string();pp.push('\0');if stat(pp.as_bytes(),&mut st)<0{return '\0';}match st.st_mode&0xF000{0x8000=>'f',0x4000=>'d',0xA000=>'l',0x6000=>'b',0x2000=>'c',0x1000=>'p',_=>'?'}}
fn fmode(p:&str,bits:u32)->bool{let mut st=Stat::default();let mut pp=p.to_string();pp.push('\0');stat(pp.as_bytes(),&mut st)>=0&&st.st_mode&bits!=0}
fn fsize(p:&str)->i64{let mut st=Stat::default();let mut pp=p.to_string();pp.push('\0');if stat(pp.as_bytes(),&mut st)<0{return 0;}st.st_size}
fn fmtime(p:&str)->i64{let mut st=Stat::default();let mut pp=p.to_string();pp.push('\0');if stat(pp.as_bytes(),&mut st)<0{return 0;}st.st_mtime}

// ── Shell struct ──────────────────────────────────────────────────────────
struct Shell {
    env:       BTreeMap<String,String>,
    aliases:   BTreeMap<String,String>,
    functions: BTreeMap<String,String>,
    last_status: i32,
    interactive: bool,
    cwd:       String,
    pid:       i64,
    ppid:      i64,
    jobs:      Vec<Job>,
    next_job:  u32,
    hist_size: usize,
    cmd_num:   usize,
    opt_errexit:   bool,
    opt_nounset:   bool,
    opt_xtrace:    bool,
    opt_noglob:    bool,
    opt_noclobber: bool,
    opt_monitor:   bool,
    plugins: BTreeMap<String,Vec<String>>,
}
struct Job { id:u32, pids:Vec<i32>, cmd:String, status:JobSt }
#[derive(PartialEq,Clone)]
enum JobSt { Run, Stop, Done(i32) }

const BUILTINS: &[&str] = &[
    "cd","pwd","echo","printf","read","export","unset","set",
    "eval","exec","exit","return","source",".","alias","unalias",
    "type","which","hash","jobs","fg","bg","wait","kill","trap",
    "true","false","test","[","[[",":","shift","getopts","local",
    "declare","typeset","readonly","let","ulimit","umask","times",
    "history","fc","pushd","popd","dirs","suspend","disown",
    "command","builtin","enable","help","logout","compgen","complete",
    "bind","shopt","time","fn","plugin",
];

impl Shell {
    fn new() -> Self {
        let mut env=BTreeMap::new();
        env.insert("PATH".to_string(),"/bin:/sbin:/usr/bin:/usr/sbin:/usr/local/bin".to_string());
        env.insert("HOME".to_string(),"/root".to_string());
        env.insert("TERM".to_string(),"xterm-256color".to_string());
        env.insert("LANG".to_string(),"en_US.UTF-8".to_string());
        env.insert("SHELL".to_string(),"/bin/qsh".to_string());
        env.insert("IFS".to_string()," \t\n".to_string());
        env.insert("PS1".to_string(),"\\[\x1b[1;32m\\]\\u@\\h\\[\x1b[0m\\]:\\[\x1b[1;34m\\]\\w\\[\x1b[0m\\]\\g\\$ ".to_string());
        env.insert("PS2".to_string(),"\\[\x1b[1;33m\\]> \\[\x1b[0m\\]".to_string());
        env.insert("PS4".to_string(),"\\[\x1b[35m\\]+ \\[\x1b[0m\\]".to_string());
        env.insert("HISTSIZE".to_string(),"50000".to_string());
        env.insert("HISTFILE".to_string(),"/root/.qsh_history".to_string());
        env.insert("EDITOR".to_string(),"vi".to_string());
        env.insert("PAGER".to_string(),"less".to_string());
        let mut cb=[0u8;4096]; let n=getcwd(&mut cb);
        let cwd=if n>0{let l=cstr_len(&cb);String::from_utf8_lossy(&cb[..l]).to_string()}else{"/".to_string()};
        let pid=getpid(); let ppid=getppid();
        let mut sh=Shell{env,aliases:BTreeMap::new(),functions:BTreeMap::new(),
            last_status:0,interactive:true,cwd,pid,ppid,
            jobs:Vec::new(),next_job:1,hist_size:0,cmd_num:0,
            opt_errexit:false,opt_nounset:false,opt_xtrace:false,
            opt_noglob:false,opt_noclobber:false,opt_monitor:true,
            plugins:BTreeMap::new()};
        sh.aliases.insert("ll".to_string(),"ls -la".to_string());
        sh.aliases.insert("la".to_string(),"ls -a".to_string());
        sh.aliases.insert("l".to_string(), "ls -CF".to_string());
        sh.aliases.insert("..".to_string(),"cd ..".to_string());
        sh.aliases.insert("...".to_string(),"cd ../..".to_string());
        sh.aliases.insert("grep".to_string(),"grep --color=auto".to_string());
        sh.aliases.insert("ls".to_string(),"ls --color=auto".to_string());
        sh.env.insert("PPID".to_string(),ppid.to_string());
        sh.env.insert("$".to_string(),pid.to_string());
        sh
    }
    fn new_child(parent: &Shell) -> Self {
        let mut sh=Shell::new();
        sh.env=parent.env.clone(); sh.aliases=parent.aliases.clone();
        sh.functions=parent.functions.clone(); sh.cwd=parent.cwd.clone();
        sh.interactive=false; sh.opt_errexit=parent.opt_errexit;
        sh.opt_nounset=parent.opt_nounset; sh.opt_xtrace=parent.opt_xtrace;
        sh.opt_noglob=parent.opt_noglob;
        sh
    }
    fn expand(&self, s: &str) -> String { expand(s, self) }
    fn expand_word(&self, w: &str) -> Vec<String> {
        let exp=self.expand(w);
        let braced=brace_expand(&exp);
        let mut res=Vec::new();
        for b in braced {
            if self.opt_noglob { res.push(b); continue; }
            let g=glob_expand(&b);
            res.extend(g);
        }
        res
    }
    fn is_builtin(&self, cmd: &str) -> bool { BUILTINS.contains(&cmd) }

    fn run_line(&mut self, line: &str) -> i32 {
        let line=line.trim();
        if line.is_empty()||line.starts_with('#'){return self.last_status;}
        if self.opt_xtrace {
            let ps4=self.env.get("PS4").cloned().unwrap_or_else(||"+ ".to_string());
            err(&alloc::format!("{}{}\n",ps4,line));
        }
        self.run_hook("pre_exec",&[line]);
        let toks=tokenize(line);
        let status=self.run_toks(&toks);
        self.last_status=status;
        self.run_hook("post_exec",&[&status.to_string()]);
        self.cmd_num+=1;
        status
    }
    fn exec_string(&mut self, s: &str) -> i32 {
        let mut st=0;
        for line in s.split('\n') {
            let line=line.trim(); if line.is_empty(){continue;}
            st=self.run_line(line);
            if self.opt_errexit&&st!=0{break;}
        }
        st
    }
    fn run_hook(&mut self, hook: &str, args: &[&str]) {
        if let Some(hooks)=self.plugins.get(hook).cloned() {
            for body in hooks {
                for (i,a) in args.iter().enumerate(){self.env.insert((i+1).to_string(),(*a).to_string());}
                self.exec_string(&body);
            }
        }
    }

    fn run_toks(&mut self, toks: &[Tok]) -> i32 {
        let mut status=0; let mut pl: Vec<Vec<Tok>>=Vec::new();
        let mut cur: Vec<Tok>=Vec::new(); let mut bg=false; let mut neg=false;
        let finish=|sh: &mut Shell, pl: &mut Vec<Vec<Tok>>, bg:bool, neg:bool|->i32{
            let s=sh.run_pipeline(pl,bg); pl.clear(); if neg{(s==0)as i32}else{s}
        };
        let mut i=0;
        while i<toks.len() {
            match &toks[i] {
                Tok::Bang=>{neg=true;i+=1;}
                Tok::Pipe|Tok::PipeErr=>{if!cur.is_empty(){pl.push(core::mem::take(&mut cur));}i+=1;}
                Tok::And=>{if!cur.is_empty(){pl.push(core::mem::take(&mut cur));}
                    if!pl.is_empty(){status=finish(self,&mut pl,bg,neg);neg=false;bg=false;}
                    if status!=0{while i<toks.len()&&!matches!(toks[i],Tok::Semi|Tok::Nl|Tok::Or){i+=1;}continue;}
                    i+=1;}
                Tok::Or=>{if!cur.is_empty(){pl.push(core::mem::take(&mut cur));}
                    if!pl.is_empty(){status=finish(self,&mut pl,bg,neg);neg=false;bg=false;}
                    if status==0{while i<toks.len()&&!matches!(toks[i],Tok::Semi|Tok::Nl|Tok::And){i+=1;}continue;}
                    i+=1;}
                Tok::Amp=>{bg=true;if!cur.is_empty(){pl.push(core::mem::take(&mut cur));}
                    if!pl.is_empty(){status=finish(self,&mut pl,true,neg);neg=false;bg=false;}i+=1;}
                Tok::Semi|Tok::Nl=>{if!cur.is_empty(){pl.push(core::mem::take(&mut cur));}
                    if!pl.is_empty(){status=finish(self,&mut pl,bg,neg);neg=false;bg=false;}i+=1;}
                t=>{cur.push(t.clone());i+=1;}
            }
        }
        if!cur.is_empty(){pl.push(cur);}
        if!pl.is_empty(){status=finish(self,&mut pl,bg,neg);}
        status
    }

    fn run_pipeline(&mut self, pl: &[Vec<Tok>], bg: bool) -> i32 {
        if pl.is_empty(){return 0;}
        if pl.len()==1{return self.run_cmd(&pl[0],bg);}
        let n=pl.len(); let mut pipes=Vec::new();
        for _ in 0..n-1{let mut fds=[0i32;2];pipe(&mut fds);pipes.push((fds[0],fds[1]));}
        let mut pids=Vec::new();
        for (idx,cmd) in pl.iter().enumerate(){
            let pid=fork(); if pid==0{
                if idx>0{dup2(pipes[idx-1].0,STDIN);}
                if idx<n-1{dup2(pipes[idx].1,STDOUT);}
                for&(r,w) in &pipes{close(r);close(w);}
                let mut sh=Shell::new_child(self); let st=sh.run_cmd(cmd,false); exit(st);
            }
            pids.push(pid);
        }
        for(r,w) in pipes{close(r);close(w);}
        let mut last=0;
        if !bg{for pid in &pids{let mut s=0i32;waitpid(*pid as i32,&mut s,0);last=wex(s);}}
        last
    }

    fn run_cmd(&mut self, toks: &[Tok], bg: bool) -> i32 {
        if toks.is_empty(){return 0;}
        if matches!(&toks[0],Tok::Fn){return self.def_fn(toks);}
        match &toks[0] {
            Tok::If=>return self.run_if(toks),
            Tok::While=>return self.run_while(toks,false),
            Tok::Until=>return self.run_while(toks,true),
            Tok::For=>return self.run_for(toks),
            Tok::Case=>return self.run_case(toks),
            Tok::LB=>return self.run_group(toks),
            Tok::Sub(body)=>{ let body=body.clone(); let pid=fork();
                if pid==0{let mut sh=Shell::new_child(self);let s=sh.exec_string(&body);exit(s);}
                if !bg{let mut s=0i32;waitpid(pid as i32,&mut s,0);return wex(s);}return 0;}
            Tok::Func=>return self.def_func(toks),
            _=>{}
        }
        let mut assigns=Vec::new(); let mut args: Vec<String>=Vec::new(); let mut redirs=Vec::new();
        for t in toks { match t {
            Tok::Assign(k,v)=>assigns.push((k.clone(),v.clone())),
            Tok::Rout(_,_,_)|Tok::Rin(_,_)|Tok::RHere(_)|Tok::RHereDoc(_,_)|Tok::RDup(_,_)=>redirs.push(t.clone()),
            Tok::Word(w)=>args.extend(self.expand_word(w)),
            _=>{}
        }}
        if args.is_empty()&&!assigns.is_empty(){
            for(k,v) in assigns{let val=self.expand(&v);self.env.insert(k,val);}return 0;}
        if args.is_empty(){return 0;}
        // Alias expansion
        if let Some(av)=self.aliases.get(&args[0]).cloned(){
            let exp=self.expand(&av); let ntoks=tokenize(&exp);
            let mut na: Vec<String>=Vec::new();
            for t in &ntoks{if let Tok::Word(w)=t{na.push(w.clone());}}
            na.extend_from_slice(&args[1..]); args=na;
        }
        if args.is_empty(){return 0;}
        let cmd=args[0].clone();
        if self.is_builtin(&cmd){
            let saved=self.apply_redirs(&redirs);
            let old: Vec<(String,String)>=assigns.iter().map(|(k,_)|(k.clone(),self.env.get(k).cloned().unwrap_or_default())).collect();
            for(k,v) in &assigns{self.env.insert(k.clone(),self.expand(v));}
            let st=self.builtin(&cmd,&args);
            self.restore_redirs(saved);
            for(k,v) in old{self.env.insert(k,v);}
            return st;
        }
        if let Some(body)=self.functions.get(&cmd).cloned(){
            let saved=self.apply_redirs(&redirs);
            for(k,v) in &assigns{self.env.insert(k.clone(),self.expand(v));}
            let mut fenv=self.env.clone();
            for(i,a) in args[1..].iter().enumerate(){fenv.insert((i+1).to_string(),a.clone());}
            fenv.insert("#".to_string(),args.len().saturating_sub(1).to_string());
            fenv.insert("0".to_string(),cmd.clone());
            let old=core::mem::replace(&mut self.env,fenv);
            let st=self.exec_string(&body); self.env=old;
            self.restore_redirs(saved); return st;
        }
        self.exec_ext(&args,&assigns,&redirs,bg)
    }

    fn exec_ext(&mut self, args:&[String], assigns:&[(String,String)], redirs:&[Tok], bg:bool)->i32 {
        let pid=fork(); if pid<0{err("qsh: fork failed\n");return 1;}
        if pid==0{
            for r in redirs{self.redir_child(r);}
            for(k,v) in assigns{self.env.insert(k.clone(),self.expand(v));}
            let env_s: Vec<String>=self.env.iter().map(|(k,v)|alloc::format!("{}={}",k,v)).collect();
            let envp: Vec<*const u8>=env_s.iter().map(|s|{let mut s2=s.clone();s2.push('\0');s2.as_ptr() as *const u8}).chain(core::iter::once(core::ptr::null())).collect();
            let cmd=&args[0];
            let paths: Vec<String>=if cmd.contains('/'){alloc::vec![cmd.clone()]}
                else{self.env.get("PATH").cloned().unwrap_or_default().split(':').map(|p|alloc::format!("{}/{}",p,cmd)).collect()};
            let mut argv_s: Vec<String>=args.iter().map(|a|{let mut s=a.clone();s.push('\0');s}).collect();
            argv_s.push("\0".to_string());
            let argv: Vec<*const u8>=argv_s.iter().map(|s|s.as_ptr() as *const u8).collect();
            for path in paths{let mut p=path.clone();p.push('\0');execve(p.as_bytes(),&argv,&envp);}
            err(&alloc::format!("qsh: {}: command not found\n",cmd)); exit(127);
        }
        if bg{
            let jid=self.next_job; self.next_job+=1;
            self.jobs.push(Job{id:jid,pids:alloc::vec![pid as i32],cmd:args.join(" "),status:JobSt::Run});
            out(&alloc::format!("[{}] {}\n",jid,pid)); 0
        } else {
            let mut s=0i32; waitpid(pid as i32,&mut s,0); wex(s)
        }
    }

    fn apply_redirs(&self, redirs: &[Tok]) -> Vec<(i32,i32)> {
        let mut saved=Vec::new(); for r in redirs{self.redir_save(r,&mut saved);} saved
    }
    fn redir_save(&self, r: &Tok, saved: &mut Vec<(i32,i32)>) {
        match r {
            Tok::Rout(fo,f,app)=>{ let fd=fo.unwrap_or(1) as i32; let sv=dup(fd); if sv>=0{saved.push((fd,sv as i32));}
                let mut p=f.clone();p.push('\0'); let fl=if *app{O_WRONLY|O_CREAT|O_APPEND}else{O_WRONLY|O_CREAT|O_TRUNC};
                let nfd=open(p.as_bytes(),fl,0o644); if nfd>=0{dup2(nfd as i32,fd);close(nfd as i32);}}
            Tok::Rin(fo,f)=>{ let fd=fo.unwrap_or(0) as i32; let sv=dup(fd); if sv>=0{saved.push((fd,sv as i32));}
                let mut p=f.clone();p.push('\0'); let nfd=open(p.as_bytes(),O_RDONLY,0); if nfd>=0{dup2(nfd as i32,fd);close(nfd as i32);}}
            Tok::RHere(s)=>{ let mut fds=[0i32;2];pipe(&mut fds); let sv=dup(STDIN);if sv>=0{saved.push((STDIN,sv as i32));}
                write(fds[1],s.as_bytes());write(fds[1],b"\n");close(fds[1]);dup2(fds[0],STDIN);close(fds[0]);}
            Tok::RHereDoc(_,c)=>{ let mut fds=[0i32;2];pipe(&mut fds); let sv=dup(STDIN);if sv>=0{saved.push((STDIN,sv as i32));}
                write(fds[1],c.as_bytes());close(fds[1]);dup2(fds[0],STDIN);close(fds[0]);}
            Tok::RDup(s,d)=>{ let sv=dup(*s as i32);if sv>=0{saved.push((*s as i32,sv as i32));} dup2(*d as i32,*s as i32);}
            _=>{}
        }
    }
    fn redir_child(&self, r: &Tok) {
        match r {
            Tok::Rout(fo,f,app)=>{ let fd=fo.unwrap_or(1) as i32; let mut p=f.clone();p.push('\0');
                let fl=if *app{O_WRONLY|O_CREAT|O_APPEND}else{O_WRONLY|O_CREAT|O_TRUNC};
                let nfd=open(p.as_bytes(),fl,0o644);if nfd>=0{dup2(nfd as i32,fd);close(nfd as i32);}}
            Tok::Rin(fo,f)=>{ let fd=fo.unwrap_or(0) as i32; let mut p=f.clone();p.push('\0');
                let nfd=open(p.as_bytes(),O_RDONLY,0);if nfd>=0{dup2(nfd as i32,fd);close(nfd as i32);}}
            Tok::RHere(s)=>{ let mut fds=[0i32;2];pipe(&mut fds);write(fds[1],s.as_bytes());write(fds[1],b"\n");close(fds[1]);dup2(fds[0],STDIN);close(fds[0]);}
            Tok::RHereDoc(_,c)=>{ let mut fds=[0i32;2];pipe(&mut fds);write(fds[1],c.as_bytes());close(fds[1]);dup2(fds[0],STDIN);close(fds[0]);}
            Tok::RDup(s,d)=>{ dup2(*d as i32,*s as i32); }
            _=>{}
        }
    }
    fn restore_redirs(&self, saved: Vec<(i32,i32)>) { for(o,sv) in saved.into_iter().rev(){dup2(sv,o);close(sv);} }

    fn run_if(&mut self, toks: &[Tok]) -> i32 {
        let mut cond=String::new(); let mut then=String::new(); let mut els=String::new();
        let mut in_c=true; let mut in_e=false; let mut d=0i32;
        for t in &toks[1..] { match t {
            Tok::Then if d==0=>in_c=false,
            Tok::Elif if d==0=>{ let s=self.exec_string(&cond); if s==0{return self.exec_string(&then);} cond.clear();then.clear();in_c=true; }
            Tok::Else if d==0=>in_e=true,
            Tok::Fi if d==0=>{ let s=self.exec_string(&cond); return if s==0{self.exec_string(&then)}else{self.exec_string(&els)}; }
            Tok::If|Tok::While|Tok::For|Tok::Case=>{d+=1;ptok(if in_e{&mut els}else if in_c{&mut cond}else{&mut then},t);}
            Tok::Fi if d>0=>{d-=1;ptok(if in_e{&mut els}else if in_c{&mut cond}else{&mut then},t);}
            _=>ptok(if in_e{&mut els}else if in_c{&mut cond}else{&mut then},t),
        }}
        let s=self.exec_string(&cond); if s==0{self.exec_string(&then)}else{self.exec_string(&els)}
    }
    fn run_while(&mut self, toks: &[Tok], inv: bool) -> i32 {
        let mut cond=String::new(); let mut body=String::new(); let mut in_b=false; let mut d=0i32;
        for t in &toks[1..] { match t {
            Tok::Do if d==0=>in_b=true, Tok::Done if d==0=>break,
            Tok::While|Tok::Until|Tok::For=>{d+=1;ptok(if in_b{&mut body}else{&mut cond},t);}
            Tok::Done if d>0=>{d-=1;ptok(if in_b{&mut body}else{&mut cond},t);}
            _=>ptok(if in_b{&mut body}else{&mut cond},t),
        }}
        let mut last=0;
        loop { let s=self.exec_string(&cond); if!(if inv{s!=0}else{s==0}){break;} last=self.exec_string(&body); }
        last
    }
    fn run_for(&mut self, toks: &[Tok]) -> i32 {
        let mut var=String::new(); let mut words=Vec::new(); let mut body=String::new();
        let mut st=0u8; let mut d=0i32;
        for t in &toks[1..] { match (st,t) {
            (0,Tok::Word(w))=>{var=w.clone();st=1;}
            (1,Tok::In)=>st=2, (1,Tok::Do)=>st=3,
            (2,Tok::Semi|Tok::Nl|Tok::Do)=>st=3,
            (2,Tok::Word(w))=>words.extend(self.expand_word(w)),
            (3,Tok::Done) if d==0=>break,
            (3,Tok::For|Tok::While|Tok::Until)=>{d+=1;ptok(&mut body,t);}
            (3,Tok::Done) if d>0=>{d-=1;ptok(&mut body,t);}
            (3,_)=>ptok(&mut body,t), _=>{}
        }}
        let mut last=0;
        for w in &words { self.env.insert(var.clone(),w.clone()); last=self.exec_string(&body); }
        last
    }
    fn run_case(&mut self, toks: &[Tok]) -> i32 {
        let mut subj=String::new(); let mut st=0u8; let mut pats: Vec<String>=Vec::new();
        let mut cbody=String::new(); let mut matched=false; let mut res=0;
        for t in &toks[1..] { match (st,t) {
            (0,Tok::Word(w))=>{subj=self.expand(w);st=1;}
            (1,Tok::In)=>st=2,
            (2,Tok::Esac)=>{ if !matched{for p in &pats{if glob_match(&subj,p)||p=="*"{res=self.exec_string(&cbody);matched=true;break;}}} break; }
            (2,Tok::Semi)=>{ if!matched{for p in &pats{if glob_match(&subj,p)||p=="*"{res=self.exec_string(&cbody);matched=true;break;}}} pats.clear();cbody.clear(); }
            (2,Tok::Word(w)) if cbody.is_empty()=>pats.push(w.trim_end_matches(')').to_string()),
            (2,_)=>ptok(&mut cbody,t), _=>{}
        }}
        res
    }
    fn run_group(&mut self, toks: &[Tok]) -> i32 {
        let mut body=String::new(); let mut d=0i32;
        for t in &toks[1..] { match t {
            Tok::LB=>{d+=1;ptok(&mut body,t);} Tok::RB if d==0=>break,
            Tok::RB=>{d-=1;ptok(&mut body,t);} _=>ptok(&mut body,t),
        }}
        self.exec_string(&body)
    }
    fn def_func(&mut self, toks: &[Tok]) -> i32 {
        let name=match toks.get(1){Some(Tok::Word(n))=>n.clone(),_=>return 1};
        let mut body=String::new(); let mut d=0i32; let mut in_b=false;
        for t in &toks[2..] { match t {
            Tok::LB if !in_b=>in_b=true, Tok::LB=>{d+=1;ptok(&mut body,t);}
            Tok::RB if d==0=>break, Tok::RB=>{d-=1;ptok(&mut body,t);}
            _=>if in_b{ptok(&mut body,t);}
        }}
        self.functions.insert(name,body); 0
    }
    fn def_fn(&mut self, toks: &[Tok]) -> i32 {
        let name=match toks.get(1){Some(Tok::Word(n))=>n.clone(),_=>{err("qsh: fn: expected name\n");return 1;}};
        let mut body=String::new(); let mut in_b=false; let mut d=0i32;
        let mut i=2;
        while i<toks.len() {
            match &toks[i] {
                Tok::LB if !in_b=>in_b=true, Tok::LB=>{d+=1;ptok(&mut body,&toks[i]);}
                Tok::RB if d==0=>break, Tok::RB=>{d-=1;ptok(&mut body,&toks[i]);}
                t=>if in_b{ptok(&mut body,t);}
            }
            i+=1;
        }
        self.functions.insert(name,body); 0
    }

    fn reap_jobs(&mut self) {
        let mut i=0;
        while i<self.jobs.len() {
            let pids=self.jobs[i].pids.clone(); let mut done=true;
            for pid in pids { let mut s=0i32; let r=waitpid(pid,&mut s,WNOHANG); if r==0{done=false;} }
            if done { let id=self.jobs[i].id; let cmd=self.jobs[i].cmd.clone();
                out(&alloc::format!("[{}]+  Done\t{}\n",id,cmd)); self.jobs.remove(i); }
            else{i+=1;}
        }
    }

    fn load_plugins(&mut self) {
        let home=self.env.get("HOME").cloned().unwrap_or_else(||"/root".to_string());
        let dp=alloc::format!("{}/.qshell/plugins\0",home);
        let fd=open(dp.as_bytes(),O_RDONLY|O_DIRECTORY,0); if fd<0{return;}
        let mut buf=alloc::vec![0u8;16384]; let mut files=Vec::new();
        loop {
            let n=getdents64(fd as i32,&mut buf); if n<=0{break;}
            let mut off=0;
            while off<n as usize {
                let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                let nb=&buf[off+19..]; let nl=nb.iter().position(|&b|b==0).unwrap_or(0);
                let name=String::from_utf8_lossy(&nb[..nl]).to_string();
                if name.ends_with(".qsh"){files.push(name);}
                if reclen==0{break;} off+=reclen;
            }
        }
        close(fd as i32); files.sort();
        let home2=self.env.get("HOME").cloned().unwrap_or_else(||"/root".to_string());
        for f in files {
            let path=alloc::format!("{}/.qshell/plugins/{}\0",home2,f);
            let pfd=open(path.as_bytes(),O_RDONLY,0); if pfd<0{continue;}
            let mut pbuf=alloc::vec![0u8;65536]; let n=read(pfd as i32,&mut pbuf);
            close(pfd as i32);
            if n>0 {
                let content=String::from_utf8_lossy(&pbuf[..n as usize]).to_string();
                self.exec_string(&content);
                out(&alloc::format!("{}[qsh]{} plugin: {}\n",C_GRAY,C_RST,f.trim_end_matches(".qsh")));
            }
        }
    }
}

fn ptok(s: &mut String, t: &Tok) {
    match t {
        Tok::Word(w)=>{s.push_str(w);s.push(' ');}
        Tok::Semi=>s.push_str("; "), Tok::Nl=>s.push('\n'),
        Tok::And=>s.push_str(" && "), Tok::Or=>s.push_str(" || "),
        Tok::Pipe=>s.push_str(" | "), Tok::Amp=>s.push(' '),
        Tok::If=>s.push_str("if "), Tok::Then=>s.push_str("then "),
        Tok::Elif=>s.push_str("elif "), Tok::Else=>s.push_str("else "),
        Tok::Fi=>s.push_str("fi"), Tok::While=>s.push_str("while "),
        Tok::Until=>s.push_str("until "), Tok::Do=>s.push_str("do "),
        Tok::Done=>s.push_str("done"), Tok::For=>s.push_str("for "),
        Tok::In=>s.push_str("in "), Tok::Case=>s.push_str("case "),
        Tok::Esac=>s.push_str("esac"), Tok::LB=>s.push_str("{ "),
        Tok::RB=>s.push('}'), Tok::Bang=>s.push_str("! "),
        Tok::Func=>s.push_str("function "), Tok::Fn=>s.push_str("fn "),
        Tok::Assign(k,v)=>s.push_str(&alloc::format!("{}={} ",k,v)),
        _=>{}
    }
}

fn wex(s: i32) -> i32 { if s&0x7F==0{(s>>8)&0xFF}else{128+(s&0x7F)} }
const WNOHANG: i32 = 1;

// ── Builtins ──────────────────────────────────────────────────────────────
impl Shell {
    fn builtin(&mut self, cmd: &str, args: &[String]) -> i32 {
        match cmd {
            ":"|"true"=>0, "false"=>1,
            "exit"=>{ let c=args.get(1).and_then(|s|s.parse().ok()).unwrap_or(self.last_status); term_restore(); exit(c); }
            "return"=>args.get(1).and_then(|s|s.parse().ok()).unwrap_or(self.last_status),
            "cd"=>{
                let dir=match args.get(1).map(|s|s.as_str()) {
                    Some("-")=>self.env.get("OLDPWD").cloned().unwrap_or_else(||"/".to_string()),
                    Some(d)=>{
                        if d.starts_with('/'){ d.to_string() }
                        else if d.starts_with('~'){
                            let h=self.env.get("HOME").cloned().unwrap_or_else(||"/root".to_string());
                            if d.len()==1{h}else{alloc::format!("{}/{}",h,&d[2..])}
                        } else { alloc::format!("{}/{}",self.cwd,d) }
                    }
                    None=>self.env.get("HOME").cloned().unwrap_or_else(||"/".to_string()),
                };
                let mut p=dir.clone();p.push('\0');
                let old=self.cwd.clone();
                if chdir(p.as_bytes())<0 { err(&alloc::format!("qsh: cd: {}: No such file or directory\n",dir)); 1 }
                else {
                    self.env.insert("OLDPWD".to_string(),old);
                    let mut buf=[0u8;4096]; let n=getcwd(&mut buf);
                    if n>0{let l=cstr_len(&buf);self.cwd=String::from_utf8_lossy(&buf[..l]).to_string();}else{self.cwd=dir;}
                    self.env.insert("PWD".to_string(),self.cwd.clone()); 0
                }
            }
            "pwd"=>{
                let p=!args.contains(&"-P".to_string());
                if p{out(&self.cwd);out("\n");}
                else{let mut b=[0u8;4096];let n=getcwd(&mut b);if n>0{let l=cstr_len(&b);out(&String::from_utf8_lossy(&b[..l]));out("\n");}}
                0
            }
            "echo"=>{
                let mut no_nl=false; let mut interp=false; let mut st=1;
                loop { match args.get(st).map(|s|s.as_str()) {
                    Some("-n")=>{no_nl=true;st+=1;} Some("-e")=>{interp=true;st+=1;}
                    Some("-E")=>{interp=false;st+=1;}
                    Some(s) if s.starts_with('-')&&s[1..].chars().all(|c|"neE".contains(c))=>{
                        for c in s[1..].chars(){match c{'n'=>no_nl=true,'e'=>interp=true,'E'=>interp=false,_=>{}}} st+=1; }
                    _=>break,
                }}
                let o=args[st..].join(" ");
                if interp{out(&decode_esc(&o));}else{out(&o);}
                if !no_nl{out("\n");} 0
            }
            "printf"=>{ if args.len()<2{err("printf: missing format\n");return 1;}
                out(&printf_fmt(&args[1],&args[2..])); 0 }
            "read"=>{
                let mut var="REPLY".to_string(); let mut prompt_s=None;
                let mut silent=false; let mut nchars=0usize; let mut delim='\n'; let mut i=1;
                while i<args.len(){match args[i].as_str(){
                    "-p"=>{i+=1;prompt_s=args.get(i).cloned();}"-s"=>silent=true,
                    "-n"=>{i+=1;nchars=args.get(i).and_then(|s|s.parse().ok()).unwrap_or(1);}
                    "-d"=>{i+=1;if let Some(d)=args.get(i){delim=d.chars().next().unwrap_or('\n');}}
                    "-r"=>{} s if !s.starts_with('-')=>{var=s.to_string();break;} _=>{}
                }i+=1;}
                let vars: Vec<String>=if i<args.len(){args[i..].to_vec()}else{alloc::vec![var]};
                if let Some(p)=prompt_s{write(STDERR,p.as_bytes());}
                let mut line=String::new(); let mut buf=[0u8;1];
                loop{let n=read(STDIN,&mut buf);if n<=0{break;}if buf[0]==delim as u8{break;}
                    if !silent{write(STDOUT,&buf[..1]);}line.push(buf[0] as char);if nchars>0&&line.len()>=nchars{break;}}
                if silent{out("\n");}
                let ifs=self.env.get("IFS").cloned().unwrap_or_else(||" \t\n".to_string());
                if vars.len()==1{self.env.insert(vars[0].clone(),line);}
                else{let f: Vec<&str>=line.split(|c:char|ifs.contains(c)).collect();
                    for(j,v) in vars.iter().enumerate(){
                        if j<f.len().saturating_sub(1){self.env.insert(v.clone(),f[j].to_string());}
                        else if j==vars.len()-1{self.env.insert(v.clone(),f.get(j..).map(|x|x.join(" ")).unwrap_or_default());}
                        else{self.env.insert(v.clone(),String::new());}
                    }}
                0
            }
            "export"=>{
                if args.len()==1{for(k,v)in&self.env{out(&alloc::format!("declare -x {}=\"{}\"\n",k,v));}return 0;}
                for a in &args[1..]{if let Some(eq)=a.find('='){self.env.insert(a[..eq].to_string(),self.expand(&a[eq+1..]));}} 0
            }
            "unset"=>{ for a in &args[1..]{self.env.remove(a.as_str());self.functions.remove(a.as_str());} 0 }
            "set"=>{
                if args.len()==1{for(k,v)in&self.env{out(&alloc::format!("{}={}\n",k,v));}return 0;}
                for a in &args[1..]{match a.as_str(){
                    "-e"=>self.opt_errexit=true,"+e"=>self.opt_errexit=false,
                    "-u"=>self.opt_nounset=true,"+u"=>self.opt_nounset=false,
                    "-x"=>self.opt_xtrace=true, "+x"=>self.opt_xtrace=false,
                    "-f"=>self.opt_noglob=true,  "+f"=>self.opt_noglob=false,
                    "-m"=>self.opt_monitor=true,  "+m"=>self.opt_monitor=false, _=>{}
                }} 0
            }
            "alias"=>{
                if args.len()==1{for(k,v)in&self.aliases{out(&alloc::format!("alias {}='{}'\n",k,v));}return 0;}
                for a in &args[1..]{if let Some(eq)=a.find('='){self.aliases.insert(a[..eq].to_string(),a[eq+1..].trim_matches('\'').to_string());}
                    else if let Some(v)=self.aliases.get(a.as_str()){out(&alloc::format!("alias {}='{}'\n",a,v));}} 0
            }
            "unalias"=>{ for a in &args[1..]{self.aliases.remove(a.as_str());} 0 }
            "source"|"."=>{
                if args.len()<2{err("source: filename required\n");return 1;}
                let path=self.expand(&args[1]); let mut p=path.clone();p.push('\0');
                let fd=open(p.as_bytes(),O_RDONLY,0);
                if fd<0{err(&alloc::format!("source: {}: not found\n",path));return 1;}
                let mut buf=alloc::vec![0u8;1<<20]; let n=read(fd as i32,&mut buf); close(fd as i32);
                if n>0{self.exec_string(&String::from_utf8_lossy(&buf[..n as usize]))}else{0}
            }
            "eval"=>{ let s=args[1..].join(" "); let e=self.expand(&s); self.run_line(&e) }
            "exec"=>{
                if args.len()<2{return 0;}
                let ca=&args[1..];
                let es: Vec<String>=self.env.iter().map(|(k,v)|alloc::format!("{}={}\0",k,v)).collect();
                let ep: Vec<*const u8>=es.iter().map(|s|s.as_ptr() as *const u8).chain(core::iter::once(core::ptr::null())).collect();
                let mut as2: Vec<String>=ca.iter().map(|a|{let mut s=a.clone();s.push('\0');s}).collect();
                as2.push("\0".to_string());
                let av: Vec<*const u8>=as2.iter().map(|s|s.as_ptr() as *const u8).collect();
                let path=&ca[0];
                if path.contains('/'){ let mut p=path.clone();p.push('\0');execve(p.as_bytes(),&av,&ep); }
                else if let Some(pe)=self.env.get("PATH"){
                    for d in pe.split(':'){let mut p=alloc::format!("{}/{}\0",d,path);execve(p.as_bytes(),&av,&ep);}
                }
                err(&alloc::format!("exec: {}: not found\n",path)); 1
            }
            "type"=>{
                for a in &args[1..]{
                    if self.is_builtin(a){out(&alloc::format!("{} is a shell builtin\n",a));}
                    else if self.aliases.contains_key(a.as_str()){out(&alloc::format!("{} is aliased to '{}'\n",a,self.aliases[a.as_str()]));}
                    else if self.functions.contains_key(a.as_str()){out(&alloc::format!("{} is a function\n",a));}
                    else{match path_find(a,self.env.get("PATH").map(|s|s.as_str()).unwrap_or("")){
                        Some(p)=>out(&alloc::format!("{} is {}\n",a,p)),None=>{err(&alloc::format!("{}: not found\n",a));return 1;}
                    }}
                } 0
            }
            "which"=>{ let mut st=0;
                for a in &args[1..]{match path_find(a,self.env.get("PATH").map(|s|s.as_str()).unwrap_or("")){
                    Some(p)=>out(&alloc::format!("{}\n",p)),None=>{err(&alloc::format!("{}: not found\n",a));st=1;}
                }} st }
            "jobs"=>{ self.reap_jobs();
                for j in &self.jobs{let st=match&j.status{JobSt::Run=>"Running",JobSt::Stop=>"Stopped",JobSt::Done(_)=>"Done"};
                    out(&alloc::format!("[{}] {} {}\t{}\n",j.id,if matches!(j.status,JobSt::Run){"+"} else{"-"},st,j.cmd));} 0 }
            "fg"|"bg"=>{ let is_fg=cmd=="fg";
                let jid=args.get(1).and_then(|s|s.trim_start_matches('%').parse::<u32>().ok()).unwrap_or_else(||self.jobs.last().map(|j|j.id).unwrap_or(0));
                if let Some(j)=self.jobs.iter().find(|j|j.id==jid){
                    let pids=j.pids.clone(); for&p in &pids{kill(p,SIGCONT);}
                    if is_fg{for p in pids{let mut s=0i32;waitpid(p,&mut s,0);}self.jobs.retain(|j|j.id!=jid);}
                } 0 }
            "wait"=>{ if args.len()==1{for j in &self.jobs{for&p in&j.pids{let mut s=0i32;waitpid(p,&mut s,0);}}self.jobs.clear();}
                else{for a in &args[1..]{let p=a.trim_start_matches('%').parse::<i32>().unwrap_or(0);let mut s=0i32;waitpid(p,&mut s,0);}} 0 }
            "kill"=>{ let mut sig=SIGTERM; let mut st=1;
                if let Some(a)=args.get(1){if a.starts_with('-'){sig=signum(&a[1..]).unwrap_or(SIGTERM);st=2;}}
                for a in &args[st..]{let p=a.trim_start_matches('%').parse::<i32>().unwrap_or(0);kill(p,sig);} 0 }
            "test"|"["=>{ let end=if cmd=="["{args.len().saturating_sub(1)}else{args.len()}; (!eval_test(&args[1..end],self)) as i32 }
            "[["=>{ let end=args.iter().rposition(|s|s=="]]").unwrap_or(args.len()); (!eval_test(&args[1..end],self)) as i32 }
            "history"=>{ out("(use Ctrl+R for history search)\n"); 0 }
            "let"=>{ let mut r=0i64; for e in &args[1..]{r=eval_arith(&self.expand(e));} (r==0) as i32 }
            "declare"|"typeset"|"local"=>{
                for a in &args[1..]{if let Some(eq)=a.find('='){let k=a[..eq].to_string();let v=self.expand(&a[eq+1..]);self.env.insert(k,v);}
                    else if !a.starts_with('-'){self.env.entry(a.clone()).or_insert_with(String::new);}} 0 }
            "readonly"=>{
                for a in &args[1..] {
                    if let Some(eq)=a.find('=') {
                        self.env.insert(a[..eq].to_string(), self.expand(&a[eq + 1..]));
                    }
                }
                0
            }
            "umask"=>{ if args.len()==1{let m=unsafe{syscall::syscall1(95,0)};unsafe{syscall::syscall1(95,m as u64)};out(&alloc::format!("{:04o}\n",m));}
                else{umask(u32::from_str_radix(&args[1],8).unwrap_or(0o022));} 0 }
            "ulimit"=>{ out("unlimited\n"); 0 }
            "trap"|"shift"|"getopts"|"hash"|"shopt"|"compgen"|"complete"|"bind"|"enable"|"fc"=>0,
            "help"=>{
                out(&alloc::format!("{}qsh v2.0{} — Qunix Shell\n\n",C_BOLD,C_RST));
                out(&alloc::format!("{}Features:{}\n",C_BBLU,C_RST));
                out("  Syntax highlighting · Autosuggestions · Context-aware completion\n");
                out("  Persistent history (Ctrl+R) · Plugin system · Native fn() syntax\n");
                out("  Job control · Recursive glob (**) · Brace expansion\n\n");
                out(&alloc::format!("{}Prompt escapes:{}\n  ",C_BBLU,C_RST));
                out("\\u user  \\h host  \\w dir  \\g git-branch  \\? status  \\x ✓/✗  \\t time\n\n");
                out(&alloc::format!("{}fn syntax:{}\n  fn greet(name) {{ echo \"Hello $name\" }}\n\n",C_BBLU,C_RST));
                out(&alloc::format!("{}Plugin hooks:{}\n  plugin hook pre_exec  'echo running: $1'\n",C_BBLU,C_RST));
                out("  Plugins auto-loaded from ~/.qshell/plugins/*.qsh\n\n"); 0 }
            "times"=>{ out("0m0.000s 0m0.000s\n0m0.000s 0m0.000s\n"); 0 }
            "suspend"=>{ kill(getpid() as i32,SIGSTOP); 0 }
            "logout"=>{ term_restore(); exit(self.last_status); }
            "disown"=>{ if let Some(a)=args.get(1){let id=a.trim_start_matches('%').parse::<u32>().unwrap_or(0);self.jobs.retain(|j|j.id!=id);}else{self.jobs.pop();} 0 }
            "pushd"=>{ let d=args.get(1).cloned().unwrap_or_else(||"/".to_string()); self.builtin("cd",&alloc::vec!["cd".to_string(),d]) }
            "popd"=>self.builtin("cd",&alloc::vec!["cd".to_string(),"-".to_string()]),
            "dirs"=>{ out(&alloc::format!("{}\n",self.cwd)); 0 }
            "command"=>{ if args.len()<2{return 0;} self.exec_ext(&args[1..],&[],&[],false) }
            "builtin"=>{ if args.len()<2{return 0;} self.builtin(&args[1].clone(),&args[1..]) }
            "time"=>{
                if args.len()<2{return 0;}
                let mut ts=[0i64;2]; clock_gettime(1,&mut ts);
                let s=self.run_cmd(&args[1..].iter().map(|s|Tok::Word(s.clone())).collect::<Vec<_>>(),false);
                let mut te=[0i64;2]; clock_gettime(1,&mut te);
                let d=(te[0]-ts[0])*1000+(te[1]-ts[1])/1_000_000;
                err(&alloc::format!("\nreal  {}m{:.3}s\nuser  0m0.000s\nsys   0m0.000s\n",d/60000,(d%60000)as f64/1000.0)); s }
            "plugin"=>{
                if args.get(1).map(|s|s.as_str())==Some("hook")&&args.len()>=4{
                    let hook=args[2].clone(); let body=args[3..].join(" ");
                    self.plugins.entry(hook).or_default().push(body); 0
                } else if args.get(1).map(|s|s.as_str())==Some("list"){
                    for(k,v)in&self.plugins{out(&alloc::format!("hook:{} ({} handlers)\n",k,v.len()));} 0
                } else if args.get(1).map(|s|s.as_str())==Some("load")&&args.len()>=3{
                    let path=args[2].clone(); let mut p=path.clone();p.push('\0');
                    let fd=open(p.as_bytes(),O_RDONLY,0);
                    if fd<0{err(&alloc::format!("plugin: {}: not found\n",path));return 1;}
                    let mut buf=alloc::vec![0u8;65536]; let n=read(fd as i32,&mut buf); close(fd as i32);
                    if n>0{self.exec_string(&String::from_utf8_lossy(&buf[..n as usize]));}; 0
                } else { err("Usage: plugin hook <name> <body>\n       plugin list\n       plugin load <file>\n"); 1 }
            }
            "fn"=>self.def_fn(&args.iter().map(|s|Tok::Word(s.clone())).collect::<Vec<_>>()),
            _=>{ err(&alloc::format!("qsh: {}: not implemented\n",cmd)); 1 }
        }
    }
}

fn decode_esc(s: &str) -> String {
    let mut o=String::new(); let mut c=s.chars();
    while let Some(ch)=c.next() { if ch=='\\'{match c.next(){
        Some('n')=>o.push('\n'),Some('t')=>o.push('\t'),Some('r')=>o.push('\r'),
        Some('a')=>o.push('\x07'),Some('b')=>o.push('\x08'),Some('f')=>o.push('\x0C'),
        Some('v')=>o.push('\x0B'),Some('\\')=>o.push('\\'),Some('\'')=>o.push('\''),
        Some('"')=>o.push('"'),Some('e')|Some('E')=>o.push('\x1B'),Some('0')=>o.push('\0'),
        Some(x)=>{o.push('\\');o.push(x);} None=>o.push('\\'),
    }} else{o.push(ch);}}
    o
}
fn printf_fmt(fmt: &str, args: &[String]) -> String {
    let mut o=String::new(); let mut c=fmt.chars().peekable(); let mut ai=0;
    while let Some(ch)=c.next() {
        if ch!='%'{o.push(if ch=='\\'{match c.next(){Some('n')=>'\n',Some('t')=>'\t',Some('\\')=>'\\',Some(x)=>x,None=>'\\'}}else{ch});continue;}
        let sp: String=c.by_ref().take_while(|x|!matches!(x,'d'|'i'|'u'|'o'|'x'|'X'|'f'|'s'|'c'|'%')).collect();
        match c.next(){
            Some('%')=>o.push('%'),
            Some('s')=>{o.push_str(args.get(ai).map(|s|s.as_str()).unwrap_or(""));ai+=1;}
            Some('d')|Some('i')=>{o.push_str(&args.get(ai).and_then(|s|s.parse::<i64>().ok()).unwrap_or(0).to_string());ai+=1;}
            Some('u')=>{o.push_str(&args.get(ai).and_then(|s|s.parse::<u64>().ok()).unwrap_or(0).to_string());ai+=1;}
            Some('x')=>{o.push_str(&alloc::format!("{:x}",args.get(ai).and_then(|s|s.parse::<u64>().ok()).unwrap_or(0)));ai+=1;}
            Some('X')=>{o.push_str(&alloc::format!("{:X}",args.get(ai).and_then(|s|s.parse::<u64>().ok()).unwrap_or(0)));ai+=1;}
            Some('o')=>{o.push_str(&alloc::format!("{:o}",args.get(ai).and_then(|s|s.parse::<u64>().ok()).unwrap_or(0)));ai+=1;}
            Some('f')=>{o.push_str(&alloc::format!("{:.6}",args.get(ai).and_then(|s|s.parse::<f64>().ok()).unwrap_or(0.0)));ai+=1;}
            Some('c')=>{o.push(args.get(ai).and_then(|s|s.chars().next()).unwrap_or('\0'));ai+=1;}
            Some(x)=>{o.push('%');o.push(x);} None=>{}
        }
    }
    o
}
fn signum(name: &str) -> Option<i32> {
    match name.to_uppercase().as_str() {
        "HUP"|"1"=>Some(SIGHUP),"INT"|"2"=>Some(SIGINT),"QUIT"|"3"=>Some(SIGQUIT),
        "KILL"|"9"=>Some(SIGKILL),"TERM"|"15"=>Some(SIGTERM),"STOP"|"19"=>Some(SIGSTOP),
        "CONT"|"18"=>Some(SIGCONT),"USR1"|"10"=>Some(SIGUSR1),"USR2"|"12"=>Some(SIGUSR2),
        "PIPE"|"13"=>Some(SIGPIPE),"CHLD"|"17"=>Some(SIGCHLD),"TSTP"|"20"=>Some(SIGTSTP),
        "WINCH"|"28"=>Some(SIGWINCH), _=>name.parse().ok(),
    }
}
fn resolve_path(&self, cmd: &str) -> Option<String> {
    if cmd.contains('/') {
        let mut p = cmd.to_string(); p.push('\0');
        let mut st = Stat::default();
        if stat(p.as_bytes(), &mut st) >= 0 && st.st_mode & 0o111 != 0 {
            return Some(cmd.to_string());
        }
        return None;
    }
    path_find(cmd, self.env.get("PATH").map(|s| s.as_str()).unwrap_or(""))
}

fn path_find(cmd: &str, path_env: &str) -> Option<String> {
    for dir in path_env.split(':') {
        let full=alloc::format!("{}/{}\0",dir,cmd);
        let mut st=Stat::default();
        if stat(full.as_bytes(),&mut st)>=0&&st.st_mode&0o111!=0 { return Some(alloc::format!("{}/{}",dir,cmd)); }
    }
    None
}

// ── Entry point ───────────────────────────────────────────────────────────
#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8, envp: *const *const u8) -> ! {
    let mut sh = Shell::new();

    // Import environment
    let mut ep = envp;
    loop {
        let p = unsafe { *ep }; if p.is_null() { break; }
        let s = unsafe { let mut l=0; while *p.add(l)!=0{l+=1;} core::slice::from_raw_parts(p,l) };
        if let Some(eq)=s.iter().position(|&b|b==b'=') {
            sh.env.insert(String::from_utf8_lossy(&s[..eq]).to_string(), String::from_utf8_lossy(&s[eq+1..]).to_string());
        }
        ep = unsafe { ep.add(1) };
    }

    // Parse argv
    let args: Vec<&[u8]> = (0..argc as usize).map(|i| unsafe {
        let p=*argv.add(i); let mut l=0; while *p.add(l)!=0{l+=1;}
        core::slice::from_raw_parts(p,l)
    }).collect();

    let mut commands: Vec<String>=Vec::new();
    let mut script:   Option<String>=None;
    let mut i=1usize;
    while i<args.len() {
        let a=args[i];
        match a {
            b"-c"=>{i+=1;if i<args.len(){commands.push(String::from_utf8_lossy(args[i]).to_string());}}
            b"-s"|b"-i"=>sh.interactive=true,
            b"--"=>{i+=1;break;}
            a if a.starts_with(b"-")=>{ for &b in &a[1..]{match b{b'e'=>sh.opt_errexit=true,b'u'=>sh.opt_nounset=true,b'x'=>sh.opt_xtrace=true,b'f'=>sh.opt_noglob=true,_=>{}}} }
            _=>{script=Some(String::from_utf8_lossy(a).to_string());i+=1;break;}
        }
        i+=1;
    }

    // -c mode
    for cmd in commands {
        let s=sh.exec_string(&cmd);
        if sh.opt_errexit&&s!=0{exit(s);}
    }

    // Script mode
    if let Some(file)=script {
        let mut p=file.clone();p.push('\0');
        let fd=open(p.as_bytes(),O_RDONLY,0);
        if fd<0{err(&alloc::format!("qsh: {}: not found\n",file));exit(1);}
        let mut buf=alloc::vec![0u8;1<<20]; let n=read(fd as i32,&mut buf); close(fd as i32);
        if n>0 {
            let content=String::from_utf8_lossy(&buf[..n as usize]).to_string();
            let content=if content.starts_with("#!"){content.splitn(2,'\n').nth(1).unwrap_or("").to_string()}else{content};
            let status=sh.exec_string(&content); exit(status);
        }
        exit(0);
    }

    // ── Interactive REPL ──────────────────────────────────────────────────
    sh.interactive=true;

    // MOTD / greeting
    let motd_fd=open(b"/etc/motd\0",O_RDONLY,0);
    if motd_fd>=0 {
        let mut buf=[0u8;4096]; let n=read(motd_fd as i32,&mut buf); close(motd_fd as i32);
        if n>0{out(&String::from_utf8_lossy(&buf[..n as usize]));}
    } else {
        out(&alloc::format!("{}╔══════════════════════════════╗\n",C_BBLU));
        out(&alloc::format!("║ {}qsh v2.0{} — Qunix Shell     {}║\n",C_BOLD,C_RST,C_BBLU));
        out(&alloc::format!("╚══════════════════════════════╝{}\n\n",C_RST));
        out(&alloc::format!("{}help{} for commands · {}Ctrl+R{} history · {}Tab{} complete\n\n",
            C_BOLD,C_RST,C_BOLD,C_RST,C_BOLD,C_RST));
    }

    // Source rc files
    let home=sh.env.get("HOME").cloned().unwrap_or_else(||"/root".to_string());
    for rc in &[".qshellrc",".qshrc",".profile"] {
        let rp=alloc::format!("{}/{}\0",home,rc);
        let rfd=open(rp.as_bytes(),O_RDONLY,0);
        if rfd>=0 {
            let mut rb=alloc::vec![0u8;65536]; let n=read(rfd as i32,&mut rb); close(rfd as i32);
            if n>0{sh.exec_string(&String::from_utf8_lossy(&rb[..n as usize]));break;}
        }
    }

    // Load plugins from ~/.qshell/plugins/
    sh.load_plugins();

    // Load history
    let hist_file=sh.env.get("HISTFILE").cloned().unwrap_or_else(||alloc::format!("{}/.qsh_history",home));
    let mut hist=History::new(&hist_file);
    hist.load();
    sh.hist_size=hist.len();

    // Enter raw mode
    term_raw();

    let mut editor=Editor::new();

    loop {
        sh.reap_jobs();

        let prompt=render_prompt(&sh,sh.last_status);
        let plen=prompt_visible_len(&prompt);
        out(&prompt);

        match editor.read_line(&mut sh, &mut hist, plen) {
            ReadResult::Eof=>{
                out("\n"); hist.save(); term_restore(); exit(sh.last_status);
            }
            ReadResult::Line(line)=>{
                let line=line.trim().to_string();
                if line.is_empty(){continue;}
                sh.hist_size=hist.len();
                let status=sh.run_line(&line);
                if sh.opt_errexit&&status!=0{hist.save();term_restore();exit(status);}
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    term_restore();
    write(STDERR, b"qsh: panic\n");
    exit(1)
}
