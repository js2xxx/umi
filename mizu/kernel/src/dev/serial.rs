use core::{fmt, mem::MaybeUninit};

use devices::dev::MmioSerialPort;
use fdt::node::FdtNode;
use ktime::Instant;
use log::Level;
use rv39_paging::{PAddr, ID_OFFSET};
use spin::{Mutex, MutexGuard, Once};

use crate::someb;

static SERIAL: Once<Mutex<MmioSerialPort>> = Once::new();

pub struct Stdout<'a>(MutexGuard<'a, MmioSerialPort>);

impl fmt::Write for Stdout<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s)
    }
}

impl Stdout<'_> {
    pub fn write_bytes(&mut self, buffer: &[u8]) {
        buffer.iter().for_each(|&byte| self.0.send(byte));
    }
}

pub fn stdout<'a>() -> Option<Stdout<'a>> {
    SERIAL.get().map(|s| Stdout(s.lock()))
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        ksync::critical(|| {
            use core::fmt::Write;
            if let Some(mut stdout) = $crate::dev::stdout() {
                write!(stdout, $($arg)*).unwrap()
            }
        })
    };
}

#[macro_export]
macro_rules! println {
    () => {
        ksync::critical(|| {
            use core::fmt::Write;
            if let Some(mut stdout) = $crate::dev::stdout() {
                stdout.write_char('\n').unwrap()
            }
        })
    };
    ($($arg:tt)*) => {
        ksync::critical(|| {
            use core::fmt::Write;
            if let Some(mut stdout) = $crate::dev::stdout() {
                writeln!(stdout, $($arg)*).unwrap()
            }
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

struct Logger(Level);

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.0
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

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
    }

    fn flush(&self) {}
}

static mut LOGGER: MaybeUninit<Logger> = MaybeUninit::uninit();

pub fn init(node: &FdtNode) -> bool {
    let mut regs = someb!(node.reg());
    let reg = someb!(regs.next());
    SERIAL.call_once(|| {
        let paddr = PAddr::new(reg.starting_address as usize);
        let base = paddr.to_laddr(ID_OFFSET);
        let mut dev = unsafe { MmioSerialPort::new(base.val()) };
        dev.init();

        let level = match option_env!("RUST_LOG") {
            Some("error") => Level::Error,
            Some("warn") => Level::Warn,
            Some("info") => Level::Info,
            Some("debug") => Level::Debug,
            Some("trace") => Level::Trace,
            _ => Level::Warn,
        };
        unsafe {
            let logger = LOGGER.write(Logger(level));
            log::set_logger(logger).unwrap();
            log::set_max_level(level.to_level_filter());
        }

        Mutex::new(dev)
    });
    true
}
