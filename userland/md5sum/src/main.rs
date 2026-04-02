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
    let mut files: Vec<String>=Vec::new(); let mut check=false; let mut i=1;
    while i<a.len(){match a[i].as_str(){"-c"=>check=true,_=>files.push(a[i].clone())}i+=1;}
    let process=|fd:i32,name:&str|{
        let mut data=Vec::new(); let mut buf=[0u8;65536];
        loop{let n=read(fd,&mut buf);if n<=0{break;}data.extend_from_slice(&buf[..n as usize]);}
        wstr(&alloc::format!("{}  {}\n",md5(&data),name));
    };
    if files.is_empty(){process(STDIN,"-");}
    else{for f in &files{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{continue;}process(fd as i32,f);close(fd as i32);}}
    exit(0)
}
fn md5(data: &[u8]) -> String {
    let s=[7u32,12,17,22,7,12,17,22,7,12,17,22,7,12,17,22,5,9,14,20,5,9,14,20,5,9,14,20,5,9,14,20,4,11,16,23,4,11,16,23,4,11,16,23,4,11,16,23,6,10,15,21,6,10,15,21,6,10,15,21,6,10,15,21];
    let kk:[u32;64]=[0xd76aa478,0xe8c7b756,0x242070db,0xc1bdceee,0xf57c0faf,0x4787c62a,0xa8304613,0xfd469501,0x698098d8,0x8b44f7af,0xffff5bb1,0x895cd7be,0x6b901122,0xfd987193,0xa679438e,0x49b40821,0xf61e2562,0xc040b340,0x265e5a51,0xe9b6c7aa,0xd62f105d,0x02441453,0xd8a1e681,0xe7d3fbc8,0x21e1cde6,0xc33707d6,0xf4d50d87,0x455a14ed,0xa9e3e905,0xfcefa3f8,0x676f02d9,0x8d2a4c8a,0xfffa3942,0x8771f681,0x6d9d6122,0xfde5380c,0xa4beea44,0x4bdecfa9,0xf6bb4b60,0xbebfbc70,0x289b7ec6,0xeaa127fa,0xd4ef3085,0x04881d05,0xd9d4d039,0xe6db99e5,0x1fa27cf8,0xc4ac5665,0xf4292244,0x432aff97,0xab9423a7,0xfc93a039,0x655b59c3,0x8f0ccc92,0xffeff47d,0x85845dd1,0x6fa87e4f,0xfe2ce6e0,0xa3014314,0x4e0811a1,0xf7537e82,0xbd3af235,0x2ad7d2bb,0xeb86d391];
    let mut a0:u32=0x67452301; let mut b0:u32=0xefcdab89; let mut c0:u32=0x98badcfe; let mut d0:u32=0x10325476;
    let mut msg=data.to_vec(); let orig_len_bits=(data.len() as u64).wrapping_mul(8);
    msg.push(0x80); while msg.len()%64!=56{msg.push(0);} msg.extend_from_slice(&orig_len_bits.to_le_bytes());
    for block in msg.chunks(64){
        let m:[u32;16]=(0..16).map(|i|u32::from_le_bytes(block[i*4..i*4+4].try_into().unwrap_or([0;4]))).collect::<Vec<_>>().try_into().unwrap_or([0;16]);
        let(mut a,mut b,mut c,mut d)=(a0,b0,c0,d0);
        for i in 0..64u32{
            let(f,g)=match i{0..=15=>((b&c)|((!b)&d),i),16..=31=>((d&b)|((!d)&c),(5*i+1)%16),32..=47=>(b^c^d,(3*i+5)%16),_=>(c^(b|(!d)),(7*i)%16)};
            let tmp=d; d=c; c=b;
            b=b.wrapping_add((a.wrapping_add(f).wrapping_add(kk[i as usize]).wrapping_add(m[g as usize])).rotate_left(s[i as usize]));
            a=tmp;
        }
        a0=a0.wrapping_add(a); b0=b0.wrapping_add(b); c0=c0.wrapping_add(c); d0=d0.wrapping_add(d);
    }
    alloc::format!("{:08x}{:08x}{:08x}{:08x}",a0.swap_bytes(),b0.swap_bytes(),c0.swap_bytes(),d0.swap_bytes())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
