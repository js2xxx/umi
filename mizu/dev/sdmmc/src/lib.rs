#![cfg_attr(not(test), no_std)]
#![feature(array_chunks)]
#![feature(associated_type_defaults)]
#![feature(int_roundings)]

mod adma;
mod imp;
mod reg;

extern crate alloc;

use alloc::{boxed::Box, vec, vec::Vec};
use core::{
    fmt,
    future::poll_fn,
    ops::Range,
    ptr::NonNull,
    sync::atomic::{
        AtomicU32, AtomicUsize,
        Ordering::{Acquire, Release},
    },
    time::Duration,
};

use async_trait::async_trait;
use devices::{block::Block, impl_io_for_block, intr::Completion};
use ksc::Error::{self, EINVAL, EIO, ENOSYS};
use sdio_host::{
    common_cmd,
    sd::{CIC, CID, CSD, OCR, RCA, SD},
    sd_cmd, Cmd,
};
use spin::Mutex;

use self::imp::{CapacityType, Inner, RespExt};

#[derive(Debug, Clone, Default)]
pub struct Data {
    pub buffer: Vec<u8>,
    pub block_shift: Option<u32>,
    pub block_count: u16,
    pub is_read: bool,

    pub bytes_transfered: usize,
}

#[derive(Debug)]
pub struct Sdmmc {
    inner: Mutex<Inner>,
    block_shift: AtomicU32,
    capacity_blocks: AtomicUsize,
}

#[derive(Clone, Copy, Default)]
pub struct SdmmcInfo {
    pub capacity_type: CapacityType,
    pub ocr: OCR<SD>,
    pub rca: RCA<SD>,
    pub cid: CID<SD>,
    pub csd: CSD<SD>,
}

impl fmt::Debug for SdmmcInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SdmmcInfo")
            .field("capacity_type", &self.capacity_type)
            .field("ocr", &self.ocr)
            .field("rca", &self.rca.address())
            .field("cid", &self.cid)
            .field("csd", &self.csd)
            .finish()
    }
}

impl Sdmmc {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        ksync::critical(|| f(&mut self.inner.lock()))
    }

    async fn cmd_with<R: RespExt>(
        &self,
        cmd: Cmd<R>,
        require_crc: bool,
        use_auto_cmd: bool,
        mut data: Option<&mut Data>,
    ) -> Result<R::Repr, Error> {
        poll_fn(|cx| {
            let cmd = common_cmd::cmd::<R>(cmd.cmd, cmd.arg);
            self.with(|s| s.send_cmd(cx, cmd, require_crc, use_auto_cmd, data.as_deref_mut()))
        })
        .await?;

        let raw = poll_fn(|cx| self.with(|s| s.get_resp(cx))).await?;
        Ok(R::from_raw(raw))
    }

    async fn cmd<R: RespExt>(&self, cmd: Cmd<R>) -> Result<R::Repr, Error> {
        self.cmd_with(cmd, false, false, None).await
    }

    async fn app_cmd<R: RespExt>(&self, rca: u16, cmd: Cmd<R>) -> Result<R::Repr, Error> {
        self.cmd(common_cmd::app_cmd(rca)).await?;
        self.cmd(cmd).await
    }

    async fn cmd_transfer<R: RespExt>(
        &self,
        cmd: Cmd<R>,
        data: &mut Data,
    ) -> Result<R::Repr, Error> {
        let rm = common_cmd::read_multiple_blocks(0).cmd;
        let wm = common_cmd::write_multiple_blocks(0).cmd;
        let is_multiblock = cmd.cmd == rm || cmd.cmd == wm;

        let resp = self.cmd_with(cmd, false, is_multiblock, Some(data)).await?;

        let (buffer, res) = poll_fn(|cx| self.with(|s| s.get_data(cx))).await;
        data.buffer = buffer;
        data.bytes_transfered = res.unwrap_or_default();

        if is_multiblock && res.is_err() {
            let _ = self.cmd(common_cmd::stop_transmission()).await;
        }
        res.map(|_| resp)
    }
}

impl Sdmmc {
    /// Creates a new [`Sdmmc`].
    ///
    /// ## Safety
    ///
    /// - The given pointer must be valid.
    /// - No other thread must have access to the given pointer. This must
    ///   remain true for the whole lifetime of this struct.
    pub unsafe fn new(base: NonNull<()>) -> Result<Self, Error> {
        Ok(Sdmmc {
            inner: Mutex::new(Inner::new(base)?),
            block_shift: Default::default(),
            capacity_blocks: Default::default(),
        })
    }

    pub fn init_bus(&self, bus_width: usize, clock_freqs: Range<u64>) -> Result<(), Error> {
        let block_shift = self.with(|s| s.init_bus(bus_width, clock_freqs))?;
        self.block_shift.store(block_shift, Release);
        Ok(())
    }

    pub async fn init(&self) -> Result<(), Error> {
        log::trace!("Start initializing the SD card");

        self.cmd(common_cmd::idle()).await?;
        ktime::sleep(Duration::from_millis(1)).await;

        let cic: CIC = self.cmd(sd_cmd::send_if_cond(1, 0xaa)).await?.into();

        if cic.pattern() != 0xaa {
            log::error!("SD card: unsupported version: {}", cic.pattern());
            return Err(ENOSYS);
        }

        if cic.voltage_accepted() & 1 == 0 {
            log::error!("SD card: unsupported voltage: {}", cic.voltage_accepted());
            return Err(ENOSYS);
        }

        let ocr = loop {
            // Initialize card
            // 3.2-3.3V
            let cmd = sd_cmd::sd_send_op_cond(true, false, true, 0x1ff);
            let ocr: OCR<SD> = self.app_cmd(0, cmd).await?.into();
            if ocr.is_busy() {
                // Still powering up
                continue;
            }
            break ocr;
        };

        if ocr.v18_allowed() && ocr.high_capacity() {
            let res = self.cmd(sd_cmd::voltage_switch()).await?;
            if res & (1 << 19) != 0 {
                return Err(EIO);
            }
            self.with(|s| s.switch_voltage())?
        }

        let capacity_type = if ocr.high_capacity() {
            CapacityType::High
        } else {
            CapacityType::Standard
        };

        let cid: CID<SD> = self.cmd(common_cmd::all_send_cid()).await?.into();
        log::info!("SD card {cid:?}");

        let rca: RCA<SD> = self.cmd(sd_cmd::send_relative_address()).await?.into();
        log::info!("SD card RCA: {}", rca.address());

        let csd: CSD<SD> = self.cmd(common_cmd::send_csd(rca.address())).await?.into();
        log::info!("SD card {csd:?}");

        self.cmd(common_cmd::select_card(rca.address())).await?;

        self.app_cmd(rca.address(), sd_cmd::set_bus_width(true))
            .await?;

        self.capacity_blocks
            .store(csd.block_count() as usize, Release);
        let info = SdmmcInfo {
            capacity_type,
            ocr,
            rca,
            cid,
            csd,
        };
        self.with(|s| s.info = info);

        Ok(())
    }

    pub fn ack_interrupt(&self, completion: &Completion) -> bool {
        self.with(|s| s.ack_interrupt(completion))
    }

    fn block(&self, block: usize) -> Result<(u32, u32), Error> {
        match self.with(|s| (s.info.capacity_type, s.block_shift)) {
            (CapacityType::Standard, shift) => {
                Ok((block.checked_shl(shift).ok_or(EINVAL)?.try_into()?, shift))
            }
            (CapacityType::High, shift) => Ok((block.try_into()?, shift)),
        }
    }
}

#[async_trait]
impl Block for Sdmmc {
    fn block_shift(&self) -> u32 {
        self.block_shift.load(Acquire)
    }

    fn capacity_blocks(&self) -> usize {
        self.capacity_blocks.load(Acquire)
    }

    fn ack_interrupt(&self, completion: &Completion) -> bool {
        self.ack_interrupt(completion)
    }

    async fn read(&self, block: usize, buf: &mut [u8]) -> Result<usize, Error> {
        log::trace!("SD card read at {block:#x}, buffer len = {:#x}", buf.len());

        let (addr, block_shift) = self.block(block)?;
        let block_count = buf.len() >> block_shift;
        let mut data = Data {
            buffer: vec![0; block_count << block_shift],
            block_shift: Some(block_shift),
            block_count: block_count.try_into().ok().unwrap_or(u16::MAX),
            is_read: true,
            bytes_transfered: 0,
        };

        let cmd = if block_count > 1 {
            common_cmd::read_multiple_blocks(addr)
        } else {
            common_cmd::read_single_block(addr)
        };
        self.cmd_transfer(cmd, &mut data).await?;

        log::trace!("SD card read {:#x} bytes", data.bytes_transfered);
        buf[..data.bytes_transfered].copy_from_slice(&data.buffer[..data.bytes_transfered]);
        Ok(data.bytes_transfered)
    }

    async fn write(&self, block: usize, buf: &[u8]) -> Result<usize, Error> {
        log::trace!("SD card write at {block:#x}, buffer len = {:#x}", buf.len());

        let (addr, block_shift) = self.block(block)?;
        let block_count = buf.len() >> block_shift;
        let mut data = Data {
            buffer: buf[..block_count << block_shift].to_vec(),
            block_shift: Some(block_shift),
            block_count: block_count.try_into().ok().unwrap_or(u16::MAX),
            is_read: false,
            bytes_transfered: 0,
        };

        let cmd = if block_count > 1 {
            common_cmd::read_multiple_blocks(addr)
        } else {
            common_cmd::read_single_block(addr)
        };
        self.cmd_transfer(cmd, &mut data).await?;

        log::trace!("SD card written {:#x} bytes", data.bytes_transfered);
        Ok(data.bytes_transfered)
    }
}
impl_io_for_block!(Sdmmc);
