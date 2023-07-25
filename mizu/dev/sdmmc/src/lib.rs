#![cfg_attr(not(test), no_std)]
#![feature(array_chunks)]
#![feature(associated_type_defaults)]
#![feature(int_roundings)]

mod adma;
mod imp;
mod reg;

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::{
    fmt,
    future::poll_fn,
    mem,
    ops::Range,
    ptr::NonNull,
    sync::atomic::{
        AtomicU32, AtomicUsize,
        Ordering::{Acquire, Release},
    },
    time::Duration,
};

use array_macro::array;
use devices::{block::Block, impl_io_for_block, intr::Completion};
use imp::RespExt;
use ksc::Error::{self, EIO, ENOSYS};
use sdio_host::{
    common_cmd,
    sd::{SDStatus, CIC, CID, CSD, OCR, RCA, SCR, SD},
    sd_cmd, Cmd,
};
use spin::Mutex;

use self::imp::Inner;
use crate::imp::CapacityType;

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
    pub scr: SCR,
    pub status: SDStatus,
}

impl fmt::Debug for SdmmcInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SdmmcInfo")
            .field("capacity_type", &self.capacity_type)
            .field("ocr", &self.ocr)
            .field("rca", &self.rca.address())
            .field("cid", &self.cid)
            .field("csd", &self.csd)
            .field("scr", &self.scr)
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
            let data = data.as_mut().map(|data| mem::take(*data));
            self.with(|s| s.send_cmd(cx, cmd, require_crc, use_auto_cmd, data))
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

    async fn cmd_read_reg<R: RespExt, const N: usize>(
        &self,
        cmd: Cmd<R>,
        buffer: &mut Vec<u8>,
    ) -> Result<[u32; N], Error> {
        let len = N * mem::size_of::<u32>();
        buffer.resize(len, 0);

        let mut data = Data {
            buffer: mem::take(buffer),
            block_shift: Some(len.ilog2()),
            block_count: 1,
            is_read: true,
            bytes_transfered: 0,
        };

        self.cmd_transfer(cmd, &mut data).await?;

        assert_eq!(data.bytes_transfered, len);
        *buffer = data.buffer;

        let mut iter = buffer.array_chunks::<{ mem::size_of::<u32>() }>();
        Ok(array![_ => u32::from_le_bytes(*iter.next().unwrap()); N])
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
        log::info!("SD card CID: {cid:?}");

        let rca: RCA<SD> = self.cmd(sd_cmd::send_relative_address()).await?.into();
        log::info!("SD card RCA: {}", rca.address());

        let csd: CSD<SD> = self.cmd(common_cmd::send_csd(rca.address())).await?.into();
        log::info!("SD card CSD: {csd:?}");

        self.cmd(common_cmd::select_card(rca.address())).await?;

        let mut buffer = Vec::new();
        // buffer.resize(512, 0);
        // let mut data = Data {
        //     buffer,
        //     block_shift: None,
        //     block_count: 1,
        //     is_read: true,
        //     bytes_transfered: 0,
        // };

        // self.cmd_transfer(common_cmd::read_single_block(0), &mut data)
        //     .await?;
        // log::trace!("{:p}", data.buffer.as_ptr());

        // assert_eq!(data.bytes_transfered, data.buffer.len());
        // assert!(data.buffer.iter().any(|&b| b != 0));
        // let mut buffer = data.buffer;
        // todo!()

        self.cmd(common_cmd::app_cmd(rca.address())).await?;
        let scr: SCR = self
            .cmd_read_reg(sd_cmd::send_scr(), &mut buffer)
            .await?
            .into();
        log::info!("SD card SCR: {scr:?}");

        self.cmd(common_cmd::app_cmd(rca.address())).await?;
        let status: SDStatus = self
            .cmd_read_reg(sd_cmd::sd_status(), &mut buffer)
            .await?
            .into();
        log::info!("SD card status: {status:?}");

        self.capacity_blocks
            .store(csd.block_count() as usize, Release);
        let info = SdmmcInfo {
            capacity_type,
            ocr,
            rca,
            cid,
            csd,
            scr,
            status,
        };
        self.with(|s| s.info = info);

        Ok(())
    }

    pub fn ack_interrupt(&self, completion: &Completion) -> bool {
        self.with(|s| s.ack_interrupt(completion))
    }
}

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

    fn read<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _: usize,
        _: &'life1 mut [u8],
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<usize, Error>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }

    fn write<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _: usize,
        _: &'life1 [u8],
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<usize, Error>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        todo!()
    }
}
impl_io_for_block!(Sdmmc);
