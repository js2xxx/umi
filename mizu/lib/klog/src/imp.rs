use core::fmt;
#[cfg(feature = "test")]
use std::io::Write;

use spin::Mutex;

pub struct Output(());

impl Output {
    pub fn write_byte(&mut self, byte: u8) {
        #[cfg(not(feature = "test"))]
        #[allow(deprecated)]
        let _ = sbi_rt::legacy::console_putchar(byte as usize);
        #[cfg(feature = "test")]
        std::io::stdout().lock().write(&[byte]).unwrap();
    }
}

impl fmt::Write for Output {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        s.bytes().for_each(|byte| self.write_byte(byte));
        Ok(())
    }
}

pub static OUTPUT: Mutex<Output> = Mutex::new(Output(()));

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::critical(|| {
            use core::fmt::Write;
            write!(*$crate::imp::OUTPUT.lock(), $($arg)*).unwrap()
        })
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::critical(|| $crate::imp::OUTPUT.lock().write_byte(b'\n'))
    };
    ($($arg:tt)*) => {
        $crate::critical(|| {
            use core::fmt::Write;
            writeln!(*$crate::imp::OUTPUT.lock(), $($arg)*).unwrap()
        })
    };
}
