use core::{fmt, mem::MaybeUninit};

use ktime_core::Instant;
use log::Level;

use crate::println;

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
        if record.level() < Level::Debug {
            println!("[{time:?}] {}: {}", record.level(), record.args())
        } else {
            let file = record.file().unwrap_or("<NULL>");
            let line = OptionU32Display(record.line());
            println!(
                "[{time:?}] {}: [{file}:{line}] {}",
                record.level(),
                record.args()
            )
        }
    }

    fn flush(&self) {}
}

static mut LOGGER: MaybeUninit<Logger> = MaybeUninit::uninit();

/// # Safety
///
/// This function should only be called once before everything else is to be
/// started up.
pub unsafe fn init(max_level: Level) {
    let logger = LOGGER.write(Logger(max_level));
    log::set_logger(logger).expect("Failed to set the logger");
    log::set_max_level(max_level.to_level_filter());
}
