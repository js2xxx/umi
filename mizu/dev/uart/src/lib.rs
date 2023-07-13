#![no_std]

use core::{
    fmt,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};

bitflags::bitflags! {
    /// Interrupt enable flags
    #[derive(Debug, Copy, Clone, Default)]
    struct IntEnFlags: u8 {
        const RECEIVED = 1;
        const SENT = 1 << 1;
        const ERRORED = 1 << 2;
        const STATUS_CHANGE = 1 << 3;
        // 4 to 7 are unused
    }
}

bitflags::bitflags! {
    /// Line status flags
    #[derive(Debug, Copy, Clone, Default)]
    struct LineStsFlags: u8 {
        const INPUT_READY = 1;
        // 1 to 4 unknown
        const OUTPUT_EMPTY = 1 << 5;
        // 6 and 7 unknown
    }
}

macro_rules! wait_for {
    ($cond:expr) => {
        while !$cond {
            core::hint::spin_loop()
        }
    };
}

pub struct Uart {
    data: AtomicPtr<u8>,
    int_en: AtomicPtr<u8>,
    fifo_ctrl: AtomicPtr<u8>,
    line_ctrl: AtomicPtr<u8>,
    modem_ctrl: AtomicPtr<u8>,
    line_sts: AtomicPtr<u8>,
}

impl Uart {
    /// Creates a new UART interface on the given memory mapped address.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the given base address really points to a
    /// serial port device.
    pub unsafe fn new(base: usize) -> Self {
        let base_pointer = base as *mut u8;
        Self {
            data: AtomicPtr::new(base_pointer),
            int_en: AtomicPtr::new(base_pointer.add(1)),
            fifo_ctrl: AtomicPtr::new(base_pointer.add(2)),
            line_ctrl: AtomicPtr::new(base_pointer.add(3)),
            modem_ctrl: AtomicPtr::new(base_pointer.add(4)),
            line_sts: AtomicPtr::new(base_pointer.add(5)),
        }
    }

    /// Initializes the memory-mapped UART.
    ///
    /// The default configuration of [38400/8-N-1](https://en.wikipedia.org/wiki/8-N-1) is used.
    pub fn init(&mut self) {
        let self_int_en = self.int_en.load(Relaxed);
        let self_line_ctrl = self.line_ctrl.load(Relaxed);
        let self_data = self.data.load(Relaxed);
        let self_fifo_ctrl = self.fifo_ctrl.load(Relaxed);
        let self_modem_ctrl = self.modem_ctrl.load(Relaxed);
        unsafe {
            // Disable interrupts
            self_int_en.write(0x00);

            // Enable DLAB
            self_line_ctrl.write(0x80);

            // Set maximum speed to 38400 bps by configuring DLL and DLM
            self_data.write(0x03);
            self_int_en.write(0x00);

            // Disable DLAB and set data word length to 8 bits
            self_line_ctrl.write(0x03);

            // Enable FIFO, clear TX/RX queues and
            // set interrupt watermark at 14 bytes
            self_fifo_ctrl.write(0xC7);

            // Mark data terminal ready, signal request to send
            // and enable auxilliary output #2 (used as interrupt line for CPU)
            self_modem_ctrl.write(0x0B);

            // Enable interrupts
            self_int_en.write(0x01);
        }
    }

    fn line_sts(&mut self) -> LineStsFlags {
        unsafe { LineStsFlags::from_bits_truncate(*self.line_sts.load(Relaxed)) }
    }

    pub fn can_send(&mut self) -> bool {
        self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY)
    }

    /// Sends a byte on the serial port.
    pub fn send(&mut self, data: u8) {
        let self_data = self.data.load(Relaxed);
        unsafe {
            match data {
                8 | 0x7F => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    self_data.write(8);
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    self_data.write(b' ');
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    self_data.write(8)
                }
                _ => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    self_data.write(data);
                }
            }
        }
    }

    pub fn can_recv(&mut self) -> bool {
        self.line_sts().contains(LineStsFlags::INPUT_READY)
    }

    /// Tries to receive a byte on the serial port.
    pub fn try_recv(&mut self) -> Option<u8> {
        let self_data = self.data.load(Relaxed);
        unsafe {
            self.line_sts()
                .contains(LineStsFlags::INPUT_READY)
                .then(|| self_data.read())
        }
    }
}

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.send(byte);
        }
        Ok(())
    }
}
