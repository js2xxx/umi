use core::{
    fmt,
    mem::MaybeUninit,
    num::NonZeroU32,
    pin::Pin,
    sync::atomic,
    task::{ready, Context, Poll},
};

use crossbeam_queue::SegQueue;
use devices::intr::Completion;
use fdt::node::FdtNode;
use futures_util::{FutureExt, Stream};
use ksync::event::{Event, EventListener};
use ktime::Instant;
use log::{Level, LevelFilter};
use rv39_paging::{PAddr, ID_OFFSET};
use spin::{Mutex, MutexGuard, Once};
use uart::Uart;

use super::{interrupts, intr::intr_man};
use crate::{someb, tryb};

struct Serial {
    device: Mutex<Uart>,
    input: SegQueue<u8>,
    input_ready: Event,
}

static SERIAL: Once<Serial> = Once::new();

pub struct Stdout<'a>(Option<MutexGuard<'a, Uart>>);

pub struct Stdin(Option<EventListener>);

impl fmt::Write for Stdout<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(serial) = &mut self.0 {
            serial.write_str(s)?;
        } else {
            s.bytes().for_each(|b| {
                #[allow(deprecated)]
                sbi_rt::legacy::console_putchar(b.into());
            })
        }
        Ok(())
    }
}

impl Stdout<'_> {
    pub fn new() -> Self {
        Stdout(SERIAL.get().map(|s| s.device.lock()))
    }

    pub fn write_bytes(&mut self, buffer: &[u8]) {
        if let Some(serial) = &mut self.0 {
            buffer.iter().for_each(|&b| serial.send(b))
        }
    }
}

impl Stdin {
    pub fn new() -> Self {
        Stdin(None)
    }

    pub async fn event() -> umio::Event {
        let Some(serial) = SERIAL.get() else { return umio::Event::HANG_UP };

        let mut listener = None;
        loop {
            if !serial.input.is_empty() {
                break umio::Event::READABLE;
            }
            match listener.take() {
                Some(listener) => listener.await,
                None => listener = Some(serial.input_ready.listen()),
            }
        }
    }
}

impl Stream for Stdin {
    type Item = u8;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(serial) = SERIAL.get() else { return Poll::Ready(None) };
        loop {
            if let Some(data) = serial.input.pop() {
                break Poll::Ready(Some(data));
            }
            match self.0 {
                Some(ref mut listener) => {
                    ready!(listener.poll_unpin(cx));
                    self.0 = None
                }
                None => self.0 = Some(serial.input_ready.listen()),
            }
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        ksync::critical(|| {
            use core::fmt::Write;
            write!($crate::dev::Stdout::new(), $($arg)*).unwrap()
        })
    };
}

#[macro_export]
macro_rules! println {
    () => {
        ksync::critical(|| {
            use core::fmt::Write;
            $crate::dev::Stdout::new().write_char('\n').unwrap()
        })
    };
    ($($arg:tt)*) => {
        ksync::critical(|| {
            use core::fmt::Write;
            writeln!($crate::dev::Stdout::new(), $($arg)*).unwrap()
        })
    };
}

struct OptionU32Display(Option<u32>);

impl core::fmt::Display for OptionU32Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(val) = self.0 {
            write!(f, "{val}")
        } else {
            write!(f, "<NULL>")
        }
    }
}

struct Logger(LevelFilter, Mutex<()>);

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.0
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        ksync::critical(|| {
            let _guard = self.1.lock();

            let time = Instant::now();
            let id = hart_id::hart_id();
            if record.level() < Level::Debug {
                println!("[{time:?}] {}#{id}: {}", record.level(), record.args())
            } else {
                let file = record.file().unwrap_or("<NULL>");
                let (_, file) = file.split_at(file.len().saturating_sub(32));
                let line = OptionU32Display(record.line());
                println!(
                    "[{time:?}] {}#{id}: [{file}:{line}] {}",
                    record.level(),
                    record.args()
                )
            }
        })
    }

    fn flush(&self) {}
}

static mut LOGGER: MaybeUninit<Logger> = MaybeUninit::uninit();

fn ack_interrupt(completion: &Completion) -> bool {
    completion();
    if let Some(serial) = SERIAL.get() {
        ksync::critical(|| {
            let mut device = serial.device.lock();
            while let Some(b) = device.try_recv() {
                serial.input.push(b);
                serial.input_ready.notify(1);
            }
        });
    }
    true
}

pub unsafe fn init_logger() {
    let level = match option_env!("RUST_LOG") {
        Some("error") => LevelFilter::Error,
        Some("warn") => LevelFilter::Warn,
        Some("info") => LevelFilter::Info,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        _ => LevelFilter::Warn,
    };
    unsafe {
        let logger = LOGGER.write(Logger(level, Mutex::new(())));
        log::set_logger(logger).unwrap();
        log::set_max_level(level);
    }
}

fn init(node: &FdtNode, stride: usize) -> bool {
    if SERIAL.is_completed() {
        return false;
    }
    let mut regs = someb!(node.reg());
    let reg = someb!(regs.next());

    let pin = someb!(interrupts(node).next().and_then(NonZeroU32::new));
    let intr_man = someb!(intr_man());

    tryb!(SERIAL.try_call_once(|| {
        let paddr = PAddr::new(reg.starting_address as usize);
        let base = paddr.to_laddr(ID_OFFSET);

        let mut dev = unsafe { Uart::new(base.cast(), stride) };
        dev.init();
        atomic::fence(atomic::Ordering::SeqCst);

        assert!(intr_man.insert(pin, ack_interrupt));

        Ok::<_, ()>(Serial {
            device: Mutex::new(dev),
            input: SegQueue::new(),
            input_ready: Default::default(),
        })
    }));
    true
}

pub fn init_ns16550a(node: &FdtNode) -> bool {
    init(node, 1)
}

pub fn init_dw_apb_uart(node: &FdtNode) -> bool {
    init(node, 4)
}
