#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn args(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d = alloc::vec![0u8; 1<<20]; let mut t = 0;
    loop { if t >= d.len() { d.resize(d.len()*2,0); } let n = read(fd, &mut d[t..]); if n<=0{break;} t+=n as usize; }
    String::from_utf8_lossy(&d[..t]).to_string()
}
fn rdfile(p: &str) -> String {
    let mut pa = p.to_string(); pa.push('\0');
    let fd = open(pa.as_bytes(), O_RDONLY, 0); if fd < 0 { return String::new(); }
    let s = rdall(fd as i32); close(fd as i32); s
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a=args(argc,argv);
    let mut do_l=false;let mut do_w=false;let mut do_c=false;let mut do_m=false;let mut do_L=false;
    let mut files: Vec<String>=Vec::new();let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-l"|"--lines"=>do_l=true,"-w"|"--words"=>do_w=true,
            "-c"|"--bytes"=>do_c=true,"-m"|"--chars"=>do_m=true,
            "-L"|"--max-line-length"=>do_L=true,"--"=>{i+=1;break;}
            s if s.starts_with('-')&&!s.starts_with("--")=>{for c in s[1..].chars(){match c{'l'=>do_l=true,'w'=>do_w=true,'c'=>do_c=true,'m'=>do_m=true,'L'=>do_L=true,_=>{}}}}
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    while i<a.len(){files.push(a[i].clone());i+=1;}
    if !do_l&&!do_w&&!do_c&&!do_m&&!do_L{do_l=true;do_w=true;do_c=true;}
    let count=|fd:i32|->(u64,u64,u64,u64,u64){
        let mut ln=0u64;let mut wn=0u64;let mut ch=0u64;let mut by=0u64;let mut mx=0u64;
        let mut inw=false;let mut cur=0u64;let mut buf=[0u8;65536];
        loop{let n=read(fd,&mut buf);if n<=0{break;}
            for &b in &buf[..n as usize]{by+=1;ch+=1;
                if b==b'\n'{ln+=1;if cur>mx{mx=cur;}cur=0;}else{cur+=1;}
                let ws=b==b' '||b==b'\t'||b==b'\n'||b==b'\r';
                if !ws&&!inw{wn+=1;inw=true;}if ws{inw=false;}
            }
        }
        (ln,wn,ch,by,mx)
    };
    let pr=|l:u64,w:u64,c:u64,m:u64,mx:u64,nm:&str|{
        if do_l{wstr(&alloc::format!("{:8}",l));}
        if do_w{wstr(&alloc::format!("{:8}",w));}
        if do_c&&!do_m{wstr(&alloc::format!("{:8}",c));}
        if do_m{wstr(&alloc::format!("{:8}",m));}
        if do_L{wstr(&alloc::format!("{:8}",mx));}
        if !nm.is_empty(){wstr(&alloc::format!(" {}",nm));}
        wstr("\n");
    };
    if files.is_empty(){let(l,w,c,m,mx)=count(STDIN);pr(l,w,c,m,mx,"");exit(0);}
    let mut tl=0u64;let mut tw=0u64;let mut tc=0u64;let mut tm=0u64;let mut tmx=0u64;
    for f in &files{
        let(l,w,c,m,mx)=if f=="-"{count(STDIN)}else{
            let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{werr(&alloc::format!("wc: {}: No such file\n",f));continue;}let r=count(fd as i32);close(fd as i32);r
        };
        pr(l,w,c,m,mx,f);tl+=l;tw+=w;tc+=c;tm+=m;if mx>tmx{tmx=mx;}
    }
    if files.len()>1{pr(tl,tw,tc,tm,tmx,"total");}
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
