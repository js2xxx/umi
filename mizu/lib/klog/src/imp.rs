use core::fmt;
#[cfg(feature = "test")]
use std::io::Write;

use spin::Mutex;

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

pub struct Stdout;

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        ksync_core::critical(|| {
            let mut serial = CONSOLE.lock();
            s.bytes().for_each(|byte| serial.write_byte(byte));
            Ok(())
        })
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::critical(|| {
            use core::fmt::Write;
            write!($crate::Stdout, $($arg)*).unwrap()
        })
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::critical(|| $crate::Stdout.write_byte(b'\n'))
    };
    ($($arg:tt)*) => {
        $crate::critical(|| {
            use core::fmt::Write;
            writeln!($crate::Stdout, $($arg)*).unwrap()
        })
    };
}
