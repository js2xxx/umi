use core::fmt;
#[cfg(feature = "test")]
use std::io::Write;

use spin::{Mutex, MutexGuard};

struct Console;

impl Console {
    pub fn write_byte(&mut self, byte: u8) {
        #[cfg(not(feature = "test"))]
        #[allow(deprecated)]
        let _ = sbi_rt::legacy::console_putchar(byte as usize);
        #[cfg(feature = "test")]
        std::io::stdout().lock().write(&[byte]).unwrap();
    }
}

static CONSOLE: Mutex<Console> = Mutex::new(Console);

pub struct Stdout<'a>(MutexGuard<'a, Console>);

impl fmt::Write for Stdout<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes(s.as_bytes());
        Ok(())
    }
}

impl Stdout<'_> {
    pub fn write_bytes(&mut self, buffer: &[u8]) {
        buffer.iter().for_each(|&byte| self.0.write_byte(byte));
    }
}

pub fn stdout<'a>() -> Stdout<'a> {
    Stdout(CONSOLE.lock())
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::critical(|| {
            use core::fmt::Write;
            write!($crate::stdout(), $($arg)*).unwrap()
        })
    };
}

#[macro_export]
macro_rules! println {
    () => {
        use core::fmt::Write;
        $crate::critical(|| $crate::stdout().write_char('\n')).unwrap()
    };
    ($($arg:tt)*) => {
        $crate::critical(|| {
            use core::fmt::Write;
            writeln!($crate::stdout(), $($arg)*).unwrap()
        })
    };
}
