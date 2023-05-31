use core::{
    fmt,
    mem::MaybeUninit,
    num::NonZeroU32,
    pin::Pin,
    sync::atomic,
    task::{ready, Context, Poll},
};

use crossbeam_queue::SegQueue;
use devices::{dev::MmioSerialPort, Interrupt};
use fdt::node::FdtNode;
use futures_util::{FutureExt, Stream};
use ksync::event::{Event, EventListener};
use ktime::Instant;
use log::Level;
use rv39_paging::{PAddr, ID_OFFSET};
use spin::{Mutex, MutexGuard, Once};

use super::intr::intr_man;
use crate::someb;

struct Serial {
    device: Mutex<MmioSerialPort>,
    input: SegQueue<u8>,
    input_ready: Event,
}

static SERIAL: Once<Serial> = Once::new();

pub struct Stdout<'a>(Option<MutexGuard<'a, MmioSerialPort>>);

pub struct Stdin(Option<EventListener>);

impl fmt::Write for Stdout<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(serial) = &mut self.0 {
            serial.write_str(s)?;
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

struct Logger(Level, Mutex<()>);

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

async fn dispatcher(intr: Interrupt) {
    loop {
        if !intr.wait().await {
            break;
        }
        if let Some(serial) = SERIAL.get() {
            ksync::critical(|| {
                let mut device = serial.device.lock();
                while let Some(b) = device.try_recv() {
                    serial.input.push(b);
                    serial.input_ready.notify(1);
                }
            })
        }
    }
}

pub fn init(node: &FdtNode) -> bool {
    let mut regs = someb!(node.reg());
    let reg = someb!(regs.next());

    let mut intrs = someb!(node.interrupts());
    let pin = someb!(intrs.next().and_then(|i| NonZeroU32::new(i as u32)));

    let intr_man = someb!(intr_man());
    let intr = someb!(intr_man.insert(pin));

    SERIAL.call_once(|| {
        let paddr = PAddr::new(reg.starting_address as usize);
        let base = paddr.to_laddr(ID_OFFSET);

        let mut dev = unsafe { MmioSerialPort::new(base.val()) };
        dev.init();
        atomic::fence(atomic::Ordering::SeqCst);

        crate::executor().spawn(dispatcher(intr)).detach();

        let level = match option_env!("RUST_LOG") {
            Some("error") => Level::Error,
            Some("warn") => Level::Warn,
            Some("info") => Level::Info,
            Some("debug") => Level::Debug,
            Some("trace") => Level::Trace,
            _ => Level::Warn,
        };
        unsafe {
            let logger = LOGGER.write(Logger(level, Mutex::new(())));
            log::set_logger(logger).unwrap();
            log::set_max_level(level.to_level_filter());
        }

        Serial {
            device: Mutex::new(dev),
            input: SegQueue::new(),
            input_ready: Default::default(),
        }
    });
    true
}
