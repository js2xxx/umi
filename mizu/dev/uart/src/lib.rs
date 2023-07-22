#![no_std]
#![feature(pointer_byte_offsets)]

use core::{fmt, ptr::NonNull};

use volatile::{
    access::{ReadOnly, Readable, Writable},
    VolatilePtr,
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
    data: VolatilePtr<'static, ()>,
    int_en: VolatilePtr<'static, ()>,
    fifo_ctrl: VolatilePtr<'static, ()>,
    // line_ctrl: VolatilePtr<'static, ()>,
    modem_ctrl: VolatilePtr<'static, ()>,
    line_sts: VolatilePtr<'static, (), ReadOnly>,
    #[cfg(feature = "uart-status")]
    usr: VolatilePtr<'static, ()>,
    stride: usize,
}

unsafe impl Send for Uart {}

impl Uart {
    /// Creates a new UART interface on the given memory mapped address.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the given base address really points to a
    /// serial port device.
    pub unsafe fn new(base: *mut (), stride: usize) -> Self {
        Self {
            data: VolatilePtr::new(NonNull::new_unchecked(base)),
            int_en: VolatilePtr::new(NonNull::new_unchecked(base.byte_add(stride))),
            fifo_ctrl: VolatilePtr::new(NonNull::new_unchecked(base.byte_add(2 * stride))),
            // line_ctrl: VolatilePtr::new(NonNull::new_unchecked(base.byte_add(3 * stride))),
            modem_ctrl: VolatilePtr::new(NonNull::new_unchecked(base.byte_add(4 * stride))),
            line_sts: VolatilePtr::new_read_only(NonNull::new_unchecked(base.byte_add(5 * stride))),
            #[cfg(feature = "uart-status")]
            usr: VolatilePtr::new(NonNull::new_unchecked(base.byte_add(31 * stride))),
            stride,
        }
    }

    unsafe fn read_reg<A: Readable>(reg: &VolatilePtr<'static, (), A>, stride: usize) -> u8 {
        match stride {
            1 => reg.map(|ptr| ptr.cast::<u8>()).read(),
            2 => reg.map(|ptr| ptr.cast::<u16>()).read() as u8,
            4 => reg.map(|ptr| ptr.cast::<u32>()).read() as u8,
            8 => reg.map(|ptr| ptr.cast::<u64>()).read() as u8,
            _ => unreachable!("invalid stride"),
        }
    }

    unsafe fn write_reg<A: Writable>(
        reg: &mut VolatilePtr<'static, (), A>,
        stride: usize,
        value: u8,
    ) {
        match stride {
            1 => reg.map(|ptr| ptr.cast::<u8>()).write(value),
            2 => reg.map(|ptr| ptr.cast::<u16>()).write(value.into()),
            4 => reg.map(|ptr| ptr.cast::<u32>()).write(value.into()),
            8 => reg.map(|ptr| ptr.cast::<u64>()).write(value.into()),
            _ => unreachable!("invalid stride"),
        }
    }

    /// Initializes the memory-mapped UART.
    ///
    /// The default configuration of [38400/8-N-1](https://en.wikipedia.org/wiki/8-N-1) is used.
    pub fn init(&mut self) {
        let stride = self.stride;
        unsafe {
            #[cfg(feature = "uart-status")]
            wait_for!(Self::read_reg(&self.usr, stride) & 0x1 == 0);

            // Disable interrupts
            Self::write_reg(&mut self.int_en, stride, 0x00);

            // // Enable DLAB
            // Self::write_reg(&mut self.line_ctrl, stride, 0x80);

            // // Set maximum speed to 38400 bps by configuring DLL and DLM
            // Self::write_reg(&mut self.data, stride, 0x03);
            // Self::write_reg(&mut self.int_en, stride, 0x00);

            // // Disable DLAB and set data word length to 8 bits
            // Self::write_reg(&mut self.line_ctrl, stride, 0x03);

            // Enable FIFO, clear TX/RX queues and
            // set interrupt watermark at 14 bytes
            Self::write_reg(&mut self.fifo_ctrl, stride, 0xC7);

            // Mark data terminal ready, signal request to send
            // and enable auxilliary output #2 (used as interrupt line for CPU)
            Self::write_reg(&mut self.modem_ctrl, stride, 0x0B);

            // Enable interrupts
            Self::write_reg(&mut self.int_en, stride, 0x01);
        }
    }

    fn line_sts(&mut self) -> LineStsFlags {
        unsafe { LineStsFlags::from_bits_truncate(Self::read_reg(&self.line_sts, self.stride)) }
    }

    pub fn can_send(&mut self) -> bool {
        self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY)
    }

    /// Sends a byte on the serial port.
    pub fn send(&mut self, data: u8) {
        let stride = self.stride;
        unsafe {
            match data {
                8 | 0x7F => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    Self::write_reg(&mut self.data, stride, 8);
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    Self::write_reg(&mut self.data, stride, b' ');
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    Self::write_reg(&mut self.data, stride, 8)
                }
                _ => {
                    wait_for!(self.line_sts().contains(LineStsFlags::OUTPUT_EMPTY));
                    Self::write_reg(&mut self.data, stride, data);
                }
            }
        }
    }

    pub fn can_recv(&mut self) -> bool {
        self.line_sts().contains(LineStsFlags::INPUT_READY)
    }

    /// Tries to receive a byte on the serial port.
    pub fn try_recv(&mut self) -> Option<u8> {
        unsafe {
            self.line_sts()
                .contains(LineStsFlags::INPUT_READY)
                .then(|| Self::read_reg(&self.data, self.stride))
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
