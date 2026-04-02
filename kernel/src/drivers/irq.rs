use spin::Mutex;
use crate::arch::x86_64::interrupts::InterruptFrame;

pub type IrqHandler = fn(&InterruptFrame);

const MAX_IRQS: usize = 16;

static HANDLERS: Mutex<[Option<IrqHandler>; MAX_IRQS]> = Mutex::new([None; MAX_IRQS]);

pub fn register(irq: u8, handler: IrqHandler) {
    if irq as usize >= MAX_IRQS { return; }
    HANDLERS.lock()[irq as usize] = Some(handler);
}

pub fn dispatch(irq: u8, frame: &InterruptFrame) {
    if irq as usize >= MAX_IRQS { return; }
    let handler = HANDLERS.lock()[irq as usize];
    if let Some(h) = handler {
        h(frame);
    }
}
