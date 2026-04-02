use alloc::collections::VecDeque;
use spin::Mutex;

const EVIOCGVERSION: u64 = 0x45_01;
const EVIOCGID:      u64 = 0x45_02;
const EVIOCGNAME:    u64 = 0x45_06;
const EVIOCGPHYS:    u64 = 0x45_07;
const EVIOCGUNIQ:    u64 = 0x45_08;
const EVIOCGBIT:     u64 = 0x45_20;
const EVIOCGABS:     u64 = 0x45_40;
const EVIOCGKEY:     u64 = 0x45_18;

const EV_SYN:  u16 = 0;
const EV_KEY:  u16 = 1;
const EV_REL:  u16 = 2;
const EV_ABS:  u16 = 3;
const EV_MSC:  u16 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InputEvent {
    pub tv_sec:  i64,
    pub tv_usec: i64,
    pub ev_type: u16,
    pub code:    u16,
    pub value:   i32,
}

static INPUT_QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());

pub fn push_key_event(code: u16, value: i32) {
    let t = crate::time::ticks();
    let mut q = INPUT_QUEUE.lock();
    q.push_back(InputEvent {
        tv_sec:  (t / 1000) as i64,
        tv_usec: ((t % 1000) * 1000) as i64,
        ev_type: EV_KEY, code, value,
    });
    q.push_back(InputEvent {
        tv_sec:  (t / 1000) as i64,
        tv_usec: ((t % 1000) * 1000) as i64,
        ev_type: EV_SYN, code: 0, value: 0,
    });
}

pub fn read_event(buf: *mut u8, count: usize) -> usize {
    let evsize = core::mem::size_of::<InputEvent>();
    if count < evsize { return 0; }
    let mut q = INPUT_QUEUE.lock();
    match q.pop_front() {
        Some(ev) => {
            unsafe { core::ptr::copy_nonoverlapping(&ev as *const _ as *const u8, buf, evsize); }
            evsize
        }
        None => 0,
    }
}

pub fn handle_input_ioctl(fd: i32, req: u64, arg: u64) -> i64 {
    let nr = req & 0xFF;
    match nr {
        v if v == (EVIOCGVERSION & 0xFF) => {
            if arg != 0 { unsafe { *(arg as *mut u32) = 0x010001; } }
            0
        }
        v if v == (EVIOCGID & 0xFF) => {
            if arg != 0 { unsafe { core::ptr::write_bytes(arg as *mut u8, 0, 8); } }
            0
        }
        v if v == (EVIOCGNAME & 0xFF) => {
            let name = b"Qunix Input Device\0";
            if arg != 0 {
                let sz = ((req >> 16) & 0x3FFF) as usize;
                let n = name.len().min(sz);
                unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), arg as *mut u8, n); }
            }
            (b"Qunix Input Device".len()) as i64
        }
        v if v == (EVIOCGBIT & 0xFF) => {
            // Report EV_KEY + EV_SYN support
            if arg != 0 {
                let sz = ((req >> 16) & 0x3FFF) as usize;
                unsafe { core::ptr::write_bytes(arg as *mut u8, 0, sz); }
                unsafe { *(arg as *mut u8) = 0x03; } // bit 0 (EV_SYN) + bit 1 (EV_KEY)
            }
            0
        }
        _ => 0,
    }
}
