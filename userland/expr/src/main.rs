#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn av(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop{if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize;}
    String::from_utf8_lossy(&d[..t]).to_string()
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = av(argc, argv);
    if a.len() < 2 { exit(1); }
    let result = eval_expr(&a[1..]);
    wstr(&result.to_string()); write(STDOUT, b"\n");
    exit(if result.is_empty() || result == "0" { 1 } else { 0 })
}
fn eval_expr(tokens: &[String]) -> String {
    if tokens.is_empty() { return "0".to_string(); }
    // Handle : (match)
    if let Some(p) = tokens.iter().position(|t| t == ":") {
        let l = eval_expr(&tokens[..p]);
        let r = &tokens[p+1..];
        if r.is_empty() { return "0".to_string(); }
        let pat = &r[0];
        // Simple prefix match
        return if l.starts_with(pat.as_str()) { l.len().to_string() } else { "0".to_string() };
    }
    // Logical or
    if let Some(p) = find_right(tokens, "|") {
        let l = eval_expr(&tokens[..p]); let r_tok = &tokens[p+1..];
        return if !l.is_empty() && l != "0" { l } else { eval_expr(r_tok) };
    }
    // Logical and
    if let Some(p) = find_right(tokens, "&") {
        let l = eval_expr(&tokens[..p]); let r_val = eval_expr(&tokens[p+1..]);
        return if (!l.is_empty()&&l!="0")&&(!r_val.is_empty()&&r_val!="0") { l } else { "0".to_string() };
    }
    // Comparison
    for op in &["=","!=","<","<=",">",">="] {
        if let Some(p) = find_right(tokens, op) {
            let l = eval_expr(&tokens[..p]); let r_val = eval_expr(&tokens[p+1..]);
            let res = match *op {
                "="  => l==r_val, "!=" => l!=r_val,
                "<"  => l.parse::<i64>().unwrap_or(0) < r_val.parse::<i64>().unwrap_or(0),
                "<=" => l.parse::<i64>().unwrap_or(0) <= r_val.parse::<i64>().unwrap_or(0),
                ">"  => l.parse::<i64>().unwrap_or(0) > r_val.parse::<i64>().unwrap_or(0),
                ">=" => l.parse::<i64>().unwrap_or(0) >= r_val.parse::<i64>().unwrap_or(0),
                _ => false,
            };
            return (res as i64).to_string();
        }
    }
    // Arithmetic
    if let Some(p) = find_right(tokens, "+") { let l=eval_expr(&tokens[..p]).parse::<i64>().unwrap_or(0); let r=eval_expr(&tokens[p+1..]).parse::<i64>().unwrap_or(0); return (l+r).to_string(); }
    if let Some(p) = find_right(tokens, "-") { let l=eval_expr(&tokens[..p]).parse::<i64>().unwrap_or(0); let r=eval_expr(&tokens[p+1..]).parse::<i64>().unwrap_or(0); return (l-r).to_string(); }
    if let Some(p) = find_right(tokens, "*") { let l=eval_expr(&tokens[..p]).parse::<i64>().unwrap_or(0); let r=eval_expr(&tokens[p+1..]).parse::<i64>().unwrap_or(0); return (l*r).to_string(); }
    if let Some(p) = find_right(tokens, "/") { let l=eval_expr(&tokens[..p]).parse::<i64>().unwrap_or(0); let r=eval_expr(&tokens[p+1..]).parse::<i64>().unwrap_or(0); return if r==0{"0".to_string()}else{(l/r).to_string()}; }
    if let Some(p) = find_right(tokens, "%") { let l=eval_expr(&tokens[..p]).parse::<i64>().unwrap_or(0); let r=eval_expr(&tokens[p+1..]).parse::<i64>().unwrap_or(0); return if r==0{"0".to_string()}else{(l%r).to_string()}; }
    // String functions
    if tokens.len() >= 4 && tokens[0]=="substr" { let s=&tokens[1]; let pos=tokens[2].parse::<usize>().unwrap_or(1).saturating_sub(1); let len=tokens[3].parse::<usize>().unwrap_or(0); return s.chars().skip(pos).take(len).collect(); }
    if tokens.len() >= 2 && tokens[0]=="length" { return tokens[1].chars().count().to_string(); }
    if tokens.len() >= 2 && tokens[0]=="index"  { let s=&tokens[1]; let t=&tokens[2]; return s.find(t.as_str()).map(|p|p+1).unwrap_or(0).to_string(); }
    tokens[0].clone()
}
fn find_right(tokens: &[String], op: &str) -> Option<usize> {
    tokens.iter().rposition(|t| t == op)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
